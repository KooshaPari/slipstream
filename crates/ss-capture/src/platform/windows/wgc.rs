//! Windows Graphics Capture (WGC): the primary Windows source.
//!
//! Per-monitor `GraphicsCaptureItem` + a free-threaded `Direct3D11CaptureFramePool`
//! (`Bgra8`, cursor embedded), drained by one worker thread into the shared [`FrameSlot`].
//! The pool's `FrameArrived` event only wakes the worker — all D3D11 work (one immediate
//! context) stays on that single thread, so no multithreaded-device locking is needed.
//!
//! 8-bit SDR only (the pool negotiates `Bgra8`); HDR arrives with the quality todo. The
//! cursor is always embedded (`IsCursorCaptureEnabled`); cursor-forward sessions keep the
//! embedded pointer until the shape channel lands (the handshake never negotiates the
//! channel on a software host today, so this is unreachable, not silent).

use super::common::{enumerate_monitors, materialize_bgra_frame, pick_monitor, FrameSlot};
use super::{CapturedFrame, Capturer};
use anyhow::{Context, Result};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use windows::{
    core::Interface,
    Foundation::{TimeSpan, TypedEventHandler},
    Graphics::{
        Capture::{
            Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
            GraphicsCaptureSession,
        },
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
        SizeInt32,
    },
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0},
            Direct3D11::{
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource,
                ID3D11Texture2D, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
                D3D11_USAGE_STAGING,
            },
            Dxgi::{IDXGIAdapter, IDXGIDevice},
        },
        System::WinRT::{
            Direct3D11::CreateDirect3D11DeviceFromDXGIDevice, RoInitialize, RO_INIT_MULTITHREADED,
        },
    },
};

/// Windows Graphics Capture source. Owns the worker thread; dropping stops it.
pub struct WgcCapturer {
    slot: Arc<FrameSlot>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    /// The materialisation half of the split capture pipeline (see `FrameHandoff`).
    materializer: Option<std::thread::JoinHandle<()>>,
    /// Optional lifetime guard handed in by the caller and released only when the capturer is
    /// dropped. The host passes its virtual-output `keepalive` here (the same contract the Linux
    /// virtual-output capturer has, `lib.rs::open_virtual_output`): on Windows that guard owns the
    /// display backend's mode-set, so holding it for the capturer's lifetime keeps the head at the
    /// session mode for the whole session instead of restoring it the instant capture attaches.
    /// Never dereferenced.
    _keepalive: Option<Box<dyn Send>>,
}

impl WgcCapturer {
    /// Capture the primary monitor.
    pub fn open() -> Result<Self> {
        Self::open_for_monitor(None)
    }

    /// Capture the monitor named by `SLIPSTREAM_CAPTURE_MONITOR` (`\\.\DISPLAY1`), else the
    /// primary head. Fails loudly when the pin names nothing on this box.
    pub fn open_for_monitor(monitor: Option<&str>) -> Result<Self> {
        Self::open_for_monitor_with_keepalive(monitor, None)
    }

    /// As [`Self::open_for_monitor`], additionally holding `keepalive` for the capturer's
    /// lifetime (the caller's virtual-output guard — see the field docs).
    pub fn open_for_monitor_with_keepalive(
        monitor: Option<&str>,
        keepalive: Option<Box<dyn Send>>,
    ) -> Result<Self> {
        // WinRT init is process-wide and idempotent (`S_FALSE` when already
        // initialized); the outcome is deliberately ignored.
        // SAFETY: no pointers; process-wide init only.
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        let monitors = enumerate_monitors()?;
        let picked = pick_monitor(&monitors, monitor)?;
        tracing::info!(
            device = %picked.device,
            width = picked.width,
            height = picked.height,
            "wgc: capturing monitor"
        );
        let item = create_capture_item_for_monitor(picked.hmon)
            .with_context(|| format!("wgc: capture item for {}", picked.device))?;
        let (device, ctx) = create_d3d11_device()?;
        let direct_device: IDirect3DDevice = {
            let dxgi: IDXGIDevice = device.cast().context("wgc: IDXGIDevice")?;
            // SAFETY: `dxgi` is our live device; the returned WinRT device wraps it
            // with its own reference, recast below for exactly this scope.
            let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
                .context("wgc: IDirect3DDevice")?;
            inspectable.cast().context("wgc: IDirect3DDevice cast")?
        };
        // Which adapter the capture device landed on decides whether WGC can hand frames over on
        // the presenting GPU or has to copy them across adapters every frame. This box has five
        // display adapters (3090 Ti, 2080 Ti, and three root-enumerated virtual displays), so
        // "the runtime's default" is not obviously the monitor's adapter. Logged, not fatal.
        if let Err(e) = log_capture_adapter(&device) {
            tracing::warn!(error = %format!("{e:#}"), "wgc: capture adapter unknown");
        }
        let size = item.Size().context("wgc: item size")?;
        // Four buffers, not the documented minimum two: the drain thread holds one frame while
        // the materialiser maps and copies it, so with two there is no headroom for the frame
        // WGC is writing, and a full pool silently stops production -- invisible to the slot
        // counters, which count slot overwrites rather than pool starvation. ~29 MB more GPU
        // memory at 2560x1440 BGRA.
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &direct_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            4,
            size,
        )
        .context("wgc: frame pool")?;
        let session = pool
            .CreateCaptureSession(&item)
            .context("wgc: capture session")?;
        session
            .SetIsCursorCaptureEnabled(true)
            .context("wgc: cursor capture")?;

        // Experiment knob, default OFF (unset keeps today's change-driven behaviour): a minimum
        // update interval makes WGC deliver on a tick instead of only on content change. Duration
        // is in 100 ns units, so 1/hz seconds = 10_000_000 / hz.
        if let Some(hz) = std::env::var("SLIPSTREAM_WGC_MIN_UPDATE_HZ")
            .ok()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|hz| *hz > 0)
        {
            let interval = TimeSpan {
                Duration: 10_000_000 / hz,
            };
            session
                .SetMinUpdateInterval(interval)
                .context("wgc: SetMinUpdateInterval")?;
            tracing::info!(hz, "wgc: minimum update interval requested");
        }

        let slot = FrameSlot::new();
        let stop = Arc::new(AtomicBool::new(false));
        // The arrival event only wakes the worker — frame processing stays on the worker
        // thread, where the immediate context is exclusively used.
        let waker = slot.clone();
        let _token = pool
            .FrameArrived(&TypedEventHandler::new(
                move |_pool: &Option<Direct3D11CaptureFramePool>,
                      _args: &Option<windows::core::IInspectable>| {
                    waker.note_arrival();
                    waker.note_frame_arrived();
                    Ok(())
                },
            ))
            .context("wgc: FrameArrived")?;
        session.StartCapture().context("wgc: StartCapture")?;

        // Two threads: the drain thread takes/releases pool frames as fast as WGC offers them, and
        // the materialiser copies the newest one out. Fusing them made the pool's consumer look
        // slow, which is what capped arrivals at ~48/s.
        let handoff = FrameHandoff::new();
        // The pool and session are COM objects; each thread holds its own reference.
        let (pool_drain, session_drain) = (pool.clone(), session.clone());
        let materializer = std::thread::Builder::new()
            .name("slipstream-wgc-copy".into())
            .spawn({
                let (slot, stop, handoff) = (slot.clone(), stop.clone(), handoff.clone());
                move || wgc_worker(pool, session, device, ctx, slot, stop, handoff)
            })
            .context("wgc: spawn materialiser")?;
        let worker = std::thread::Builder::new()
            .name("slipstream-wgc".into())
            .spawn({
                let (slot, stop, handoff) = (slot.clone(), stop.clone(), handoff.clone());
                move || wgc_drain_worker(pool_drain, session_drain, slot, stop, handoff)
            })
            .context("wgc: spawn worker")?;
        Ok(WgcCapturer {
            slot,
            stop,
            worker: Some(worker),
            materializer: Some(materializer),
            _keepalive: keepalive,
        })
    }
}

impl Drop for WgcCapturer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.slot.note_arrival();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        if let Some(w) = self.materializer.take() {
            let _ = w.join();
        }
    }
}

impl Capturer for WgcCapturer {
    fn backend_name(&self) -> &'static str {
        "wgc"
    }

    fn next_frame(&mut self) -> Result<CapturedFrame> {
        self.slot.take_blocking()
    }

    fn try_latest(&mut self) -> Result<Option<CapturedFrame>> {
        if let Some(dead) = self.slot.dead_error() {
            anyhow::bail!("wgc worker died: {dead}");
        }
        Ok(self.slot.take())
    }

    fn supports_arrival_wait(&self) -> bool {
        true
    }

    fn wait_arrival(&mut self, deadline: std::time::Instant) {
        self.slot.wait_arrival(deadline);
    }

    fn set_active(&mut self, active: bool) {
        if !active {
            self.slot.flush();
        }
    }

    fn is_alive(&self) -> bool {
        self.slot.is_alive()
    }

    fn set_cursor_forward(&mut self, on: bool) {
        // No shape channel yet: the pointer stays embedded so it is never silently lost.
        // Unreachable today (the handshake never grants the cursor channel to a host whose
        // encoder cannot blend), logged loudly if it ever fires.
        if on {
            tracing::warn!(
                "wgc: cursor-forward requested but the pointer stays embedded (no shape channel yet)"
            );
        }
    }

    fn telemetry(&self) -> super::CaptureTelemetry {
        self.slot.telemetry()
    }
}

/// Create the per-monitor WGC item via the `IGraphicsCaptureItemInterop` COM
/// interop (the `HMONITOR` factory windows-rs does not project as a static).
fn create_capture_item_for_monitor(
    hmon: windows::Win32::Graphics::Gdi::HMONITOR,
) -> Result<GraphicsCaptureItem> {
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::Win32::System::WinRT::RoGetActivationFactory;
    // SAFETY: activation of the system capture-item factory by contract name; the
    // returned interop keeps itself alive, and `CreateForMonitor` hands us a
    // reference-counted item for exactly this scope.
    unsafe {
        let interop: IGraphicsCaptureItemInterop = RoGetActivationFactory(
            &windows::core::HSTRING::from("Windows.Graphics.Capture.GraphicsCaptureItem"),
        )
        .context("wgc: capture interop factory")?;
        interop
            .CreateForMonitor::<_, GraphicsCaptureItem>(hmon)
            .context("wgc: CreateForMonitor")
    }
}

/// Create the capture D3D11 device (BGRA-capable for the WGC interop) + immediate context.
fn create_d3d11_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut ctx = None;
    let mut level = D3D_FEATURE_LEVEL_11_0;
    // All out-params are live `Option` locals written synchronously; the adapter and
    // feature-level params are `None` (default adapter, runtime's choice).
    // SAFETY: see above; the returned interfaces are reference-counted.
    unsafe {
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut level),
            Some(&mut ctx),
        )
        .context("wgc: D3D11CreateDevice")?;
    }
    Ok((
        device.context("wgc: no D3D11 device")?,
        ctx.context("wgc: no immediate context")?,
    ))
}

/// Report the DXGI adapter the capture device was created on.
///
/// Hang-safe by construction: it reads the adapter the device already holds
/// (`IDXGIDevice::GetAdapter`) and never walks the adapter chain. `EnumAdapters1` is not called
/// anywhere in this path because it blocks for 45+ s on this machine (faulted virtual display
/// driver in the chain).
fn log_capture_adapter(device: &ID3D11Device) -> Result<()> {
    let dxgi: IDXGIDevice = device.cast().context("wgc: IDXGIDevice")?;
    // SAFETY: `dxgi` is our live device; `GetAdapter` is a synchronous getter and the returned
    // adapter is reference-counted for exactly this scope.
    let adapter: IDXGIAdapter = unsafe { dxgi.GetAdapter() }.context("wgc: GetAdapter")?;
    // SAFETY: `adapter` is live; `GetDesc` returns its description struct by value (it carries
    // the LUID, so no IDXGIAdapter1 projection is needed).
    let desc = unsafe { adapter.GetDesc() }.context("wgc: GetDesc")?;
    let name = String::from_utf16_lossy(&desc.Description)
        .trim_end_matches('\0')
        .to_string();
    tracing::info!(
        adapter = %name,
        luid_low = desc.AdapterLuid.LowPart,
        luid_high = desc.AdapterLuid.HighPart,
        dedicated_vram_mb = desc.DedicatedVideoMemory / (1024 * 1024),
        "wgc: capture D3D11 adapter"
    );
    Ok(())
}

/// One-deep, replace-newest handoff from the drain thread to the materialisation thread.
///
/// Why this exists: materialising a frame (D3D11 staging map plus row copies) used to run on the
/// same thread that drains the pool, so while a frame was being copied the pool was neither
/// drained nor released and the capture session saw a slow consumer. A slow consumer looks exactly
/// like a slow producer at the `FrameArrived` callback -- the ambiguity the cadence counters
/// exposed (~48 arrivals/s with no pool overflow). The drain thread now only takes and releases
/// frames; the copy happens on another thread.
struct FrameHandoff {
    frame: std::sync::Mutex<Option<Direct3D11CaptureFrame>>,
    ready: std::sync::Condvar,
}

impl FrameHandoff {
    fn new() -> Arc<Self> {
        Arc::new(FrameHandoff {
            frame: std::sync::Mutex::new(None),
            ready: std::sync::Condvar::new(),
        })
    }

    /// Replace whatever is parked (the previous frame is released right here).
    fn put(&self, frame: Direct3D11CaptureFrame) {
        let mut guard = self.frame.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(frame);
        self.ready.notify_one();
    }

    /// Wake a waiter without a new frame (shutdown).
    fn wake(&self) {
        self.ready.notify_all();
    }

    /// Take the newest parked frame, waiting until `deadline` when none is ready.
    fn take_until(&self, deadline: std::time::Instant) -> Option<Direct3D11CaptureFrame> {
        let mut guard = self.frame.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(frame) = guard.take() {
                return Some(frame);
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return None;
            }
            let (next, _) = self
                .ready
                .wait_timeout(guard, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            guard = next;
        }
    }
}

/// Drain thread: wait for arrivals, take every queued frame, park the freshest and release the
/// rest. Does no copying -- that is the point of the split.
fn wgc_drain_worker(
    pool: Direct3D11CaptureFramePool,
    _session: GraphicsCaptureSession,
    slot: Arc<FrameSlot>,
    stop: Arc<AtomicBool>,
    handoff: Arc<FrameHandoff>,
) {
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        slot.wait_arrival(std::time::Instant::now() + std::time::Duration::from_millis(500));
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let mut pending: Option<Direct3D11CaptureFrame> = None;
        loop {
            match pool.TryGetNextFrame() {
                Ok(frame) => {
                    slot.note_try_next(true);
                    // Assigning drops the older frame, releasing its pool slot immediately.
                    pending = Some(frame);
                }
                Err(_) => {
                    slot.note_try_next(false);
                    break;
                }
            }
        }
        if let Some(frame) = pending {
            handoff.put(frame);
        }
    }
    handoff.wake();
}

/// Materialisation thread: take the newest parked frame and copy it out. Owns the immediate
/// context (not thread-safe), which is why there is exactly one of these.
fn wgc_worker(
    _pool: Direct3D11CaptureFramePool,
    _session: GraphicsCaptureSession,
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    slot: Arc<FrameSlot>,
    stop: Arc<AtomicBool>,
    handoff: Arc<FrameHandoff>,
) {
    let mut staging: Option<(ID3D11Texture2D, u32, u32)> = None;
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let Some(frame) =
            handoff.take_until(std::time::Instant::now() + std::time::Duration::from_millis(500))
        else {
            continue;
        };
        match materialize_frame(&frame, &device, &ctx, &mut staging) {
            Ok(captured) => slot.publish(captured),
            Err(e) => {
                slot.fail(format!("{e:#}"));
                break;
            }
        }
    }
}

/// Materialize one WGC frame: surface → `ID3D11Texture2D` → staging copy → mapped rows.
fn materialize_frame(
    frame: &Direct3D11CaptureFrame,
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    staging: &mut Option<(ID3D11Texture2D, u32, u32)>,
) -> Result<CapturedFrame> {
    let size: SizeInt32 = frame.ContentSize().context("wgc: ContentSize")?;
    let (w, h) = (size.Width.max(1) as u32, size.Height.max(1) as u32);
    // `GetInterface` queries the surface for `ID3D11Texture2D` through COM; the returned
    // interface keeps the texture alive for exactly this scope.
    let texture: ID3D11Texture2D = {
        use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
        let access: IDirect3DDxgiInterfaceAccess =
            frame.Surface().context("wgc: Surface")?.cast()?;
        // SAFETY: `access` is a live interop object wrapping this frame's surface; the
        // returned texture is reference-counted for exactly this scope.
        unsafe { access.GetInterface().context("wgc: texture")? }
    };
    let stage = match staging {
        Some((tex, sw, sh)) if *sw == w && *sh == h => tex.clone(),
        _ => {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            // `desc` is a plain value struct; the out-texture is returned, not
            // borrowed. Initial data is `None` (uninitialized staging is fine — every
            // byte is overwritten by the copy below before it is read).
            let mut tex = None;
            // SAFETY: out-param is a live local written synchronously; the texture is
            // reference-counted for exactly this scope.
            unsafe {
                device
                    .CreateTexture2D(&desc, None, Some(&mut tex))
                    .context("wgc: staging")?;
            }
            let tex: ID3D11Texture2D = tex.context("wgc: no staging texture")?;
            *staging = Some((tex.clone(), w, h));
            tex
        }
    };
    // Both textures are live D3D11 objects on this thread's exclusive immediate
    // context; `CopyResource` reads the whole source into the same-size staging texture.
    // The `ID3D11Resource` upcasts keep both objects alive for the call.
    let stage_res: ID3D11Resource = stage.cast().context("wgc: staging resource")?;
    let texture_res: ID3D11Resource = texture.cast().context("wgc: source resource")?;
    // SAFETY: live objects, exclusive immediate context, same-size textures.
    unsafe {
        ctx.CopyResource(&stage_res, &texture_res);
    }
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // The staging texture is CPU-readable; `Map` hands us a pointer + pitch valid
    // until the matching `Unmap` below, on this thread only.
    // SAFETY: staging is CPU-readable and exclusively owned here; unmapped below.
    unsafe {
        ctx.Map(&stage_res, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .context("wgc: Map")?;
    }
    let frame_out =
        materialize_bgra_frame(mapped.pData as *const u8, mapped.RowPitch as usize, w, h);
    // Unmaps the texture mapped above; the materialized bytes were copied out.
    // SAFETY: balances the `Map` above on the same object.
    unsafe {
        ctx.Unmap(&stage_res, 0);
    }
    Ok(frame_out)
}
