# Session topology and virtual display

Status: design only. **Deferred** until the streaming core is finished.
No code in this document is scheduled. The purpose is to record the shape of three
capabilities while the current architecture is fresh, so the eventual implementation
does not have to rediscover it.

Everything below is grounded in the tree as it stands. Where the answer is not known,
the text says `unknown - needs investigation` rather than guessing.

---

## 0. What the current architecture already implies

Five facts constrain all three capabilities. They are worth stating once.

**A host already serves several concurrent sessions.** `DEFAULT_MAX_CONCURRENT = 4`
(`crates/slipstream-host/src/protocol/slipstream1.rs:250`), with a semaphore taken after
the QUIC handshake so a full host still accepts the connection and leaves the client
waiting on a live path (`slipstream1.rs:379-429`). Each session gets its own virtual
output plus its own encoder and shares the host-lifetime input, audio, and mic services
(`slipstream1.rs:380-382`). "One client to many hosts" is therefore a *client* problem,
not a host-capacity problem.

**A host assumes one live session per client identity.** Admission keeps a live-session
table keyed by the client certificate fingerprint (`crates/ss-vdisplay/src/display/admission.rs:21-45`).
A same-identity reconnect is treated as a zombie of the *same* client and is preempted
so it cannot land on a second display (`admission.rs:124-135`). The conflict policy among
*different* clients is `separate | join | steal | reject` (`admission.rs:64-91`,
`crates/ss-vdisplay/src/display/policy.rs:104`). Note the comment at
`crates/ss-vdisplay/src/display/session.rs:158`: "The host serves one session at a time"
is true of the *session environment* (process-wide `WAYLAND_DISPLAY` etc.), not of the
session count. That distinction matters for capability 1.

**The client is one process per connect.** A connect resolves a single `HostTarget`
and `ConnectPlan` (`crates/ss-client-core/src/runtime/orchestrate.rs:27-77`), then spawns
`slipstream-session` as a child and supervises a JSON stdout contract of
`Ready | Error | Ended | Window | Exited` (`orchestrate.rs:562-579`, `650-724`).
`exec_session` is the same argv without the supervisor (`orchestrate.rs:729-737`).
There is no client-side notion of a second host inside one session process.

**Display lifetime is registry-owned with a lease.** The registry, not the capturer,
holds the real backend keepalive, so a display can outlive the session that created it
(`crates/ss-vdisplay/src/display/registry.rs:2`, `14-18`, `74-75`). The capturer holds a
generation-stamped lease; dropping the lease does not tear the output down, dropping the
pooled entry does. Reuse is gated by the compositor session epoch
(`crates/ss-vdisplay/src/display/session.rs:18-36`).

**The control plane is a flat, extensible message set.** `MSG_RECONFIGURE` through
`MSG_PHASE_REPORT` occupy `0x01..0x32`, clipboard `0x40..0x44`, cursor `0x50..0x51`
(`crates/slipstream-core/src/net/quic/control.rs:190-212`, `557-607`, `865-868`).
New capability messages fit in the unused ranges without a wire-version bump; new
*handshake* fields ride the documented trailing-byte discipline of `Hello`/`Welcome`
(`crates/slipstream-core/src/net/quic/handshake.rs:13-95`, `128-208`).

One naming hazard: `SessionTopology` at
`crates/slipstream-host/src/session/plan.rs:214` is a *process* topology
(`SingleProcess`) and has nothing to do with this document. Any new type should avoid
reusing that name.

---

## 1. One client to many hosts

### User story

The operator sits at one desk with a laptop client. Behind it are three hosts: a gaming
box, a work workstation, and a build machine. Today each is a separate connect with a
separate window and a separate teardown. The operator wants to hold all three reachable
at once and move between them the way a KVM switches inputs, without the reconnect
ceremony and without waiting for a virtual display to be created again.

### The two shapes, and a recommendation

**Switch** - one active session at a time; the others are held warm on their hosts.
**Multiplex** - all sessions decode at once, one is presented.

Multiplex is the wrong first target. It multiplies decoder instances, GPU surfaces,
audio streams, and input focus arbitration on the *client*, and it buys nothing the
operator asked for: they see one screen at a time. Switch is also already half-built,
because N `slipstream-session` children can exist simultaneously and the only thing
missing is a front-end that owns more than one.

Recommendation: **switch, with an explicit warm tier per host**, and no client-side
media multiplexing until there is evidence anyone wants split view.

### What "warm" costs on the host

This is the crux, and today there is exactly one tier.

| Tier | Host cost | Exists today |
| --- | --- | --- |
| Active | capture + encode + send at client mode | yes |
| Warm-encode | encoder running at a reduced rate | **no** - no low-fps encode mode exists |
| Warm-idle | display kept, encoder closed, capture closed | partly, via keep-alive linger (`registry.rs:87-135`, `lifecycle.rs:17-63`) |
| Cold | display torn down | yes (`registry.rs:152`) |

A "warm" session whose encoder is still running costs a full NVENC/VAAPI session slot
and full pixel-rate encode. On this box the encoder is pixel-rate bound, not bitrate
bound, so a 60 fps encode does not get cheaper because the client is not watching.
Encoding at 5 fps would be cheap, but there is no code path that does that: the stream
loop is driven by capture cadence and the encoder is opened per session. Introducing a
low-fps mode is a real change to the data plane, not a flag.

So the honest recommendation is **warm-idle**: keep the virtual display (already
supported by the keep-alive policy), drop the encoder and the session permit, and let a
switch trigger a fresh handshake against a display that already exists. That is where
the latency win comes from anyway - display creation is the slow part, not encoder open.

### Required protocol changes

For warm-idle, none strictly. The switch is: close the old session with the deliberate
quit code (`QUIT_CLOSE_CODE`, `handshake.rs:102`) - **not** a bare disconnect, so the
display lingers instead of being torn down - then connect to the next host. The cost is
a full handshake and encoder open on each switch.

To make warm meaningful without the display already being up, add a small control
message pair rather than stretching `Hello`:

- `MSG_SESSION_SUSPEND` (`0x60`): client asks the host to release the encoder and
  session permit while keeping the display and the admission entry alive.
- `MSG_SESSION_RESUME` (`0x61`): client asks to rebuild capture + encode against the
  still-live display. Ack carries the delivered mode, reusing the `Reconfigured` shape
  (`control.rs:25-27`).

`Hello` gains one trailing enum `warm_preference: {cold, idle, low_fps}` only if
low-fps ever ships. Until then it is not needed and should not be added, because the
`Hello` tail budget after `display_hdr` is capped at `HDR_META_BODY_LEN - 1` bytes
(`handshake.rs:86-94`) and it is already crowded.

### Host-side state machine

Reuse the existing pieces rather than inventing a session lifecycle:

```
                hello
   [Admitting] ---------> [Active]
        |                    |  ^
        | reject             |  | MSG_SESSION_RESUME
        v                    v  |
     [Closed]           [Suspended]      <- new: encoder + permit released,
                             |              display lease + LiveGuard retained
                             | suspend timeout
                             v
                       [Lingering]  -> [TornDown]   (existing keep-alive policy)
```

The permit is the important part. `serve_session` owns an `OwnedSemaphorePermit`
(`serve.rs:36`, `170`, `205-214`). Suspending must **drop that permit** and a resume must
re-acquire it - exactly the pattern already used when parking an unpaired knock. Without
this, four snoozing hosts would occupy every slot and a fifth host would be unreachable
even though nobody is watching anything.

The `LiveGuard` from `admission.rs:139-165` must *stay* held while suspended: the
identity still owns the display, and a second client must not be able to `steal` it out
from under a suspended session. This is a semantic change - `LiveSession` currently means
"streaming", and it would now mean "owns". That is a one-field change (`state`) but it
touches the conflict tests, and `steal` must decide what it does to a suspended session.
Recommendation: `steal` preempts suspended sessions first, before active ones.

`SessionPlan::topology` stays `SingleProcess`; nothing here changes process shape.

### Client-side state machine

```
  [Idle] --connect--> [Connecting] --ready--> [Active]
                          |                     |  |
                          | error               |  | switch away
                          v                     |  v
                       [Failed]                 | [Warm]  (child alive, control
                                                |          channel open, no video)
                                                |   | switch to
                                                |   v
                                                +-> [Active]
```

Implementation-wise, "Warm" is a `slipstream-session` child in a suspended state. The
front-end needs to own a map of `HostId -> CancelHandle + SessionEvent channel`, not the
single `CancelHandle` it holds today (`orchestrate.rs:634-642`). Audio must be explicitly
muted on non-active sessions; the client has one output device, and today nothing
prevents two children from both playing.

Input arbitration falls out of this: only the active child has a live input plane, so the
"which host gets my keystrokes" question is answered by which session is active. That is
a genuine advantage of switch over multiplex.

### Failure and security analysis

- **Host down while warm.** The warm child observes its QUIC idle timeout
  (`crates/slipstream-core/src/net/quic/endpoint.rs:17`) and emits `SessionEvent::Ended`
  followed by `Exited`; the front-end must show it as offline without killing the other
  children. Today a single child exiting ends the front-end's busy state, so the map
  above is required.
- **Display accumulation.** Each warm-idle host now holds a kept display for the linger
  duration (`policy.rs:39-77`). Switching across three hosts can therefore light up
  three virtual outputs on three different machines, which is fine, but on a *single*
  host with `max_displays` set low (`policy.rs:219`) a rapid switch loop can race the
  linger timer. The registry already refuses reuse across session epochs and reaps
  dead-epoch entries (`registry.rs:204`, `session.rs:18-36`); this is a timeout tuning
  question, not new machinery.
- **Security.** No new trust surface: each host still authenticates the client by its
  certificate fingerprint, and the client still pins each host. The one new risk is
  accidental: a suspended session keeps a display alive on a machine the user has walked
  away from. The host-side privacy question ("is anyone still attached?") is answered by
  the existing events stream, but the console should mark suspended sessions distinctly
  so an operator can see three idle strands rather than one.
- **Resource multiplication is real.** Three warm-idle hosts is three kept displays.
  Three *low-fps* hosts would be three encoders, which on a single GPU is not three
  times the cost, it is one contended encoder. That asymmetry is the main argument for
  warm-idle.

### Effort estimate

- Front-end that owns several children and switches between them, no host changes,
  sessions torn down on switch: **3-5 days**. This is almost entirely client work and is
  the version I would ship.
- Warm-idle with `MSG_SESSION_SUSPEND/RESUME`, permit release, and suspended `LiveGuard`
  semantics: **1.5-2.5 weeks** including the admission test matrix.
- Low-fps warm encode: **3-4 weeks**, because it is a data-plane cadence change, and I
  would not do it without a measured latency win from the warm-idle version.

### Smallest useful version

A host switcher in the front-end. N hosts kept as connectable entries; switching closes
the current session with `QUIT_CLOSE_CODE` (so its display lingers) and connects the
next. No protocol change, no host change, one new client-side owner for a set of
children. It removes the reconnect-from-scratch feel for the operator's actual desk
setup and it is reversible if the idea turns out to be uninteresting.

---

## 2. Daisy chaining with a block toggle

### User story

Host A is reachable from the client; host B is not (different network, or B is behind
A's link). The operator connects to A and sees B's screen. The "block toggle" is a
switch on A that either passes the chained stream through or stops it.

### Reading the toggle

The operator's phrase admits two distinct mechanisms, and they should be designed as
two features even if they share a UI:

1. **Relay gate.** A forwards B's media and control planes to the client. The toggle
   stops forwarding - the client sees a clearly-labelled placeholder ("relay blocked"),
   the session stays up, the control channel stays responsive.
2. **Source switch.** A presents *its own* local screen instead of the chained one. The
   toggle is a `source` change, not a mute. This is strictly more useful and strictly
   harder, because it is a live renegotiation of where pixels come from.

The privacy kill switch the operator is describing is (1) with the control channel kept
open. If the control channel is also dropped, it is just a disconnect and should be
expressed as such rather than as a toggle.

### Authority and pairing

The cleanest model: **the client is paired with A only.** A is separately paired with B
as an ordinary client. The client never learns B's key. A terminates the client's QUIC
connection and originates a second QUIC connection to B.

That means the client cannot cryptographically verify that the pixels came from B. A is
a man-in-the-middle by construction, which is exactly what a relay is. Two mitigations:

- A forwards B's host certificate fingerprint in a `Welcome` extension; the client
  pins it on first use and can show "relayed from B (fingerprint ...)". This does not
  stop a malicious A from lying, but it does make a leaked/incorrect relay visible and
  makes accidental cross-wiring detectable.
- Pairing B to A uses the existing delegated-approval path
  (`serve.rs:113-211`, `native_pairing`), so B's operator sees and approves A once,
  and can unpair it from B's console without touching the client.

The alternative - client paired with both A and B, B trusting the client directly - is
worse: it requires B to be reachable for pairing, which defeats the point, and it
halves the benefit because the client could then connect to B if it were reachable.

### Key and identity delegation across hops

A originates its own QUIC connection to B, so there is no key delegation on the wire.
What *does* need delegation is **identity for input and auditing**: B's admission layer
keys sessions on the certificate fingerprint (`admission.rs:57-59`). To B, the session
belongs to A. If B's `mode_conflict = reject` and A already holds B's only display, the
relayed session and A's own session conflict in a way neither operator asked for.

Recommended: A's relay connection carries an attested client label in `Hello::name`
plus a signed relay envelope. If we do not want signatures in v1, the honest fallback is
that B sees A as the client and the console says so. Do **not** pretend B can see the
real client's identity without a cryptographic claim: `unknown - needs investigation`
whether the native pairing store can carry a small attestation without a new trust root.

### Input routing

Three planes exist and each needs an explicit answer:

- **Client -> A:** unchanged, the normal input datagrams (`serve.rs:446-491`).
- **A -> B:** A must re-emit input as *its own* client input to B. This means A's host
  process needs a client-input path, i.e. part of `ss-client-core` running inside the
  host. That is a new dependency direction and should be called out as the largest
  structural cost of this feature.
- **A's own local input vs relayed input:** if someone is physically at A, both the
  local pointer and the relayed pointer drive B. There is no seat arbitration in the
  current injector model - it applies whatever it is given. The block toggle is not
  enough; a "who owns the seat" arbiter (local last-input-wins, or hard-exclusive) is a
  prerequisite, and it does not exist today. This is the detail most likely to sink a
  naive implementation.

Latency compounds: client -> A -> B -> A -> client is two network legs and two encode/
decode round trips if A re-encodes. Pass-through of B's encoded stream avoids the second
decode/encode but then A cannot composite anything, cannot apply its own bitrate
control, and cannot present a local screen without a full switch. For v1, **re-encode
at A** is simpler and the latency cost is one extra decode+encode, which on a LAN is
tens of milliseconds. Whether that is acceptable for interactive use is
`unknown - needs investigation` and should be measured before building.

### Host-side state machine (A)

```
 [Idle] --client hello--> [Relaying] <--> [Blocked]
                              |   \           |
                              |    \-> [LocalSource]  (present A's own screen)
                              |                 |
                    upstream lost              |
                              v                 v
                       [UpstreamDown] -----> [TornDown]
```

- `Relaying`: upstream QUIC to B up, B's control + media forwarded.
- `Blocked`: upstream still up (or parked), client gets a placeholder frame and a
  control message saying why; no media leaves A.
- `LocalSource`: A stops forwarding and starts its own capture pipeline for its own
  display. The client's session is the *same* session with a different source, so the
  client must accept a source change without a reconnect.
- `UpstreamDown`: B vanished. The client must see an explicit state, not a frozen last
  frame. Frozen video is the single worst failure mode of every remote-desktop relay and
  it is avoidable here because A owns the control channel.

### Client-side state machine

Mostly unchanged; the additions are display-only:

```
  [Active] --relay-control--> [Active: relayed | blocked | local]
                    |
                    +-- upstream-lost --> [Active: degraded banner, retry]
```

The client should not need to know that A is relaying at all except for the banner and
the fingerprint pin. That is the right user-visible contract: a relay is an
implementation detail of A, and the moment the client has relay-specific state machines
the feature has leaked.

### Failure and security analysis

- **B dies.** If A forwards B's connection close faithfully, the client would tear down
  its session to A, which is wrong. A must *absorb* the upstream close and translate it
  into `UpstreamDown` on the client's control channel. This is the one place the design
  must be explicit; the existing typed close codes (`reject.rs`, `handshake.rs:102-110`)
  are not sufficient because they all mean "end this session".
- **A dies.** The client loses everything unless it is also paired with B. Document that
  clearly: a relay is not a redundancy mechanism.
- **Privacy.** A sees every pixel by construction. The toggle is a policy control by A
  for A's owner, not a guarantee to the client. If the client's threat model includes A,
  a relay cannot help, and the doc should say so rather than implying otherwise.
- **Security of the second hop.** A is a client of B and must be treated as one: its own
  certificate, its own pairing, revocable from B's console. Do not special-case relay
  connections in B's admission, or the relay becomes an unrevocable super-client.
- **Amplification.** A relay makes A send media it did not capture, potentially to
  several clients. The `max_concurrent` semaphore counts A's *client-facing* sessions,
  not upstreams; a naive relay can fan one upstream into four downstreams and multiply
  the GPU cost while the semaphore still reads 1. Upstreams need their own accounting.

### Effort estimate

- Static relay, no toggle, re-encode at A: **2-3 weeks**. The input path is the risk.
- Add `Blocked` (relay gate) with an explicit client state: **+3-5 days**.
- Add `LocalSource` live switch: **+1-2 weeks**, because it is a live source
  renegotiation and touches capture lifetime.
- Seat arbitration for local-vs-relayed input: **+1 week**, and it is a prerequisite, not
  a nice-to-have.
- Full: **5-7 weeks**, with the input arbiter and the source switch dominating.

### Smallest useful version

A static relay with a local admin kill switch: A is configured with one upstream B; the
client pairs with A and pins B's fingerprint forwarded in `Welcome`; the kill switch
closes the upstream and swaps A's session to its own local capture. No `Blocked`
placeholder, no per-hop UI, no client-side relay state. If that is not enough for real
use, the feature is probably not worth finishing.

---

## 3. Virtual display management, Parsec-style

### User story

The operator wants to "add screens, fake screens on top of those representing monitor".
That is two different requests and both are legitimate.

### Reading (a): extra heads beyond the physical monitors

Create additional virtual displays, each with its own mode, position, and refresh,
which the OS sees as real heads. This is the Parsec behaviour: the remote client gets a
desktop sized to *its* panel and the physical monitor is untouched.

The pieces already exist and are close to sufficient:

- Policy already carries `max_displays` (`policy.rs:219`), `topology`
  (`policy.rs:85`), `identity` (`policy.rs:120`), and a manual `layout` of
  `(x, y)` positions keyed by identity slot (`policy.rs:153-157`).
- The registry already pools outputs with a generation stamp, a refcount, and a
  linger, and serves them by identity slot (`registry.rs:24-50`, `87-135`).
- `GET /display/state` already reports each display's slot, backend, mode, state,
  sessions, client, group, display index, position, identity slot, and topology
  (`crates/slipstream-host/src/mgmt/display.rs:337-360`).
- `PUT /display/layout` already persists arrangement (`display.rs:414-437`).

What is missing is *creation and destruction as first-class objects* rather than as a
side effect of a session. Today a display exists because a session acquired it. The
operator cannot say "keep a 3440x1440 at -3440,0 named `left` and give it to whoever
connects", except indirectly through the layout block and a preset.

Required management API additions (the route table is at `mgmt.rs:185-286`):

- `POST /display/displays` - `{name, mode, position, backend_preference?}` returns the
  created slot.
- `PATCH /display/displays/{slot}` - mode and position of a kept display.
- `DELETE /display/displays/{slot}` - explicit teardown, distinct from
  `POST /display/release` which only reaps kept ones (`display.rs:362-384`).
- `GET /display/monitors` stays the ground truth for what the OS actually sees
  (`display.rs:263-268`).

Host state machine: the existing `lifecycle::State` (`Active | Lingering | Pinned`,
`lifecycle.rs:17-28`) plus a name-keyed pin. Today `Pinned` is only reached through the
keep-alive policy when the last session leaves (`policy.rs:39-77`, `lifecycle.rs:103-116`).
The addition is a pin that never needed a session to exist, and that a name can address.
A named display must survive reconnect (it is pinned), survive compositor restart by
being rebuilt in a new session epoch (`session.rs:18-36`), and be destroyed explicitly.

Naming and placement: identity slot is already the join key between policy layout and
registry entries. Names should be an alias on top, not a second identity, or the two will
drift.

### Reading (b): a synthetic head over a physical monitor

A fake head that represents a real monitor. Three concrete uses, all plausible:

1. The monitor is not capturable directly (a headless box, a locked-down output), so a
   synthetic head is created at the same geometry and used as the capture surface.
2. Multiple clients each see a different slice of one physical panel.
3. A client sees a monitor at a mode the physical panel cannot do.

Mechanically this is a virtual output whose geometry is *derived* from a physical
monitor rather than chosen by the client. On Windows the IDD backend already publishes
modes and plugs a connector (`windows_idd.rs:191-224`); on Linux it is the existing
virtual-output backends with a monitor-derived mode.

Slice-per-client is the hard one: it requires capture of a rectangle plus per-client
cropping, and the current capture path captures a whole output
(`capture_virtual_output` at `crates/slipstream-host/src/capture.rs:179-280`). Cropping
after capture is cheap and probably the right v1; per-client capture surfaces are not.

What happens when the physical monitor's mode changes is the defining problem of
reading (b), and it is not solved today:

- The Windows mirror backend reads the head's mode, sets it to the client's mode, and
  holds a `ModeRestore` guard that puts the original back on drop
  (`crates/ss-vdisplay/src/display/windows.rs:119-156`, `175-206`).
- That is display-wide: every session sees the same head, and concurrent sessions at
  different modes are last-writer-wins. The comment at `windows.rs:16-18` says the
  admission layer keeps them apart, which is true for distinct clients but not for two
  sessions that both mirror.
- A synthetic head layered on a physical monitor makes this worse, because now two
  owners (the physical panel's user and the synthetic head's geometry) can both change
  the mode, and `ModeRestore` will "helpfully" revert the other owner's change.

For reading (b) the rule should be: **a derived head never sets the physical mode**.
It observes and mirrors. Setting the mode stays the mirror backend's job and stays
single-owner.

### The lifetime contract, and the bug class

The contract is stated in three places and is the thing to respect:

- `crates/ss-vdisplay/src/lib.rs:13`: the output is an owned keepalive whose `Drop`
  releases it; there is no explicit destroy.
- `registry.rs:2`: the registry is the owner of the lifetime, so a display can outlive
  the session.
- `registry.rs:74-75`: the capturer gets a *lease*, and the real output outlives it per
  the keep-alive policy.

On Windows this is expressed as `VirtualOutput.keepalive = Box<ModeRestore>`
(`windows.rs:147-155`). Two failure modes follow directly, and both are real:

1. **Early drop.** If the capture session is rebuilt (capture-loss rebuild, mid-stream
   reconfigure) and the `VirtualOutput` is dropped at the wrong point, `ModeRestore`
   fires and the physical panel snaps back to its pre-session mode *mid-stream*. The
   stream then continues at the wrong mode, or the encoder rebuild fails. The guard must
   outlive the capture session, not the capture attempt.
2. **Mis-owned adoption.** The IDD backend explicitly distinguishes `owned` and adopts a
   head a previous session leaked (`windows_idd.rs:93-99`, `166-181`), and only unplugs
   what it created (`227-252`). Reading (b) creates exactly the situation where a
   synthetic head is *not* ours - it represents someone's panel - so it must be
   `owned = false` or it will unplug a physical display on disconnect. Getting this
   wrong once is a support incident, not a bug.

There is a third, subtler one worth writing down now: `WindowsMirrorDisplay` reports
`DisplayOwnership::External` (`windows.rs:5-7`, `151`), so none of the registry's
keep-alive pooling applies. That is correct for a mirror, and it means the *only*
thing holding the restore guard is the session's own `VirtualOutput`. Any new
"display exists independently of a session" object must therefore either move mirror
outputs into the registry (changing the ownership split, and the `External` contract) or
accept that named mirror displays cannot outlive a session. I would keep them
session-scoped and say so.

### Effort estimate

- (a) Named, persistently created virtual displays on Linux with the three API routes,
  reusing the registry and policy layout: **1.5-2 weeks**. Most of it is API surface,
  OpenAPI regeneration, and the console.
- (a) on Windows via IDD: **+1 week**, because plug/unplug is already there but
  persistence, naming, and mode-change reconciliation are not.
- (b) derived head, single client, no slicing, never sets the physical mode: **2 weeks**.
- (b) per-client slices of one panel: **+2-3 weeks**, plus a real risk to
  `ModeRestore` correctness.
- Full: **4-6 weeks**, with the lifetime/ownership work (not the capture work) being the
  bulk.

### Smallest useful version

Reading (a) only: the three management routes over the existing registry and layout,
with a name as an alias for an identity slot, no new capture path, no Windows IDD
persistence. That gives the operator "add a screen" without touching the mode-restore
contract at all. Reading (b) should wait until the ownership question above has a
written answer, because it extends the guard's lifetime across two owners and that is
where the bug class lives.

---

## 4. Sequencing

**Build 1 first.** It is the cheapest by an order of magnitude (front-end work only, no
protocol change, no host change), it is immediately useful for the operator's actual
desk, and it produces real usage data about whether warm sessions are worth paying for.
It also forces the client to grow a multi-session owner, which is a prerequisite for
almost anything else in this document.

**Build 3(a) second.** Named virtual displays are the missing vocabulary the other two
features keep reaching for: "which screen am I switching to", "what does B's session
attach to", "what does the operator mean by add a screen". It is a natural extension of
policy plus registry, both of which already support the primitives, and it has no
cross-host trust surface. Doing it second means 1 can already name the thing it switches.

**Do 2 last, and do not start it without the seat arbiter.** It is the likely trap:

- It is the only one of the three that introduces a man-in-the-middle *by design*, so it
  needs a written authority and revocation story that does not exist yet.
- Its most-requested variant ("block the relay") is ambiguous in the operator's own
  framing - gate versus source switch - and building the wrong one wastes weeks.
- It needs the client stack (input and decode) inside the *host*, a dependency
  direction the tree does not currently have.
- The most visible failure mode, frozen video when the upstream dies, is exactly what
  the current typed-close model cannot express.

Its value is also narrow: one operator, two boxes. Two client windows, or a VPN plus a
direct pair, already solves it for most setups. If the operator still wants it after 1
and 3(a) exist, the design above is the shape; if the requirement quietly evaporates,
that is the expected outcome and not a failure.

The trap in 3 is subtler and worth repeating: reading (b) looks like a small extension
of the existing virtual-display code and is not, because it puts a second owner on a
physical head whose mode-restore guard is a single-owner RAII object
(`windows.rs:175-206`). Treat (b) as a separate project with its own lifetime design,
not as a variant of (a).

The ordering stands after the measurement below, with one amendment: a host-side
race in the same-identity admission path (section 5) should be fixed before any of
this ships, because capability 1 depends on "one display per client" actually holding.

---

## 5. Concurrency reality check

Measured 2026-11-16 against `<nixos-lan-ip>:9777` (nixos host, idle), release
`slipstream-probe.exe`, 1920x1080@144 HEVC at 8 Mbit/s, 15 s per probe, launched in
parallel by PowerShell jobs. Read-only apart from the probe sessions: no config change,
no restart.

### Method, and why it had to be one identity

The probe reads its client certificate from `$HOME/.config/slipstream/client-{cert,key}.pem`
(`clients/probe/src/args.rs`, `load_or_create_identity`), so every probe on this desk
presents the same fingerprint. A fresh identity is **not** usable for this test: it
parks at the pairing gate and never reaches a Welcome. The host log confirms it
(`session ended with error ... error=pairing request not approved within 180s`), so
`require_pairing` is on and a distinct-identity concurrency test would need an armed
pairing window, which is a config change and out of scope.

That is itself a constraint worth recording: the honest limit of this measurement is
"how many sessions one paired client identity can hold", not "how many different
clients the host can serve".

### Results

| Probes launched | Sessions that streamed | Per-probe summary |
| --- | --- | --- |
| 1 (baseline) | 1 | `frames=1568 mb=1 lat_p50=22.9ms` |
| 2 | 1 | survivor `frames=1700 mb=1 lat_p50=10.2ms`; the other got a Welcome then ended ~4.5 s in with `frames=0` |
| 4 | **3** | `frames=1675 mb=2 p50=11.1ms`, `frames=1610 mb=1 p50=9.8ms`, `frames=1630 mb=1 p50=10.2ms`; the fourth `frames=0` |
| 5 | 1 | survivor `frames=1703 mb=2 p50=10.9ms`; four ended with `frames=0` |

No probe was refused. There was no `REJECT_BUSY` close, no "host busy" reason, no
parked connection. The fourth probe in the 4-probe run and four of five in the 5-probe
run received a Welcome and then had their session end early with zero frames.

### Host-side view

`display/state` could not be read for this run. The token is readable on the host
(`~/.config/slipstream/mgmt-token`, 64 chars, never printed), but the checked-in helper
(`ssh/mgmt-api-auth.sh`) targets `https://127.0.0.1:47992`, which is the cookie-authenticated
console and answers `401 {"error":"unauthorized"}` to a bearer token. The API on 47990
is the one that takes the token, and writing a new helper was outside this task's
one-file limit. The host journal was used instead, which is better evidence anyway
because it is the host's own account:

- 2-probe run: one KWin output created (`node_id=101`), then reused.
- 4-probe run: `node_id=86` created, plus `node_id=108` and `node_id=110` created within
  2.2 s. **Three virtual outputs live at once**, matching the three frames-bearing probes.
- 5-probe run: only `node_id=98` and `node_id=94` appear, with 98 reused three times.

So the host genuinely serves concurrent sessions, and did here with three at once on
one identity.

### What this means

Two things, and they point in opposite directions.

**Good news for capability 1.** Host capacity is real and is not the constraint. Three
simultaneous 1080p144 encodes at ~107-113 fps each, with the client side running three
software decoders, held frame rate. A warm-idle multi-host switcher is not asking the
host for anything it cannot do, and four permits (`slipstream1.rs:250`) is enough
headroom for a desk with three hosts.

**Bad news for the admission contract.** `preempt_same_identity` does not serialize a
client to one session. The preempt at `handshake.rs:141-153` reads the live-session
table, and a session only registers once its pipeline is up; a burst of simultaneous
Hellos from one identity therefore all pass the check before any of them registers. The
4-probe run shows the consequence precisely: one output was *reused* (`node_id=86`,
the documented reconnect behaviour) while two more were *created* (`108`, `110`) - three
displays for one client, which is the exact "thrown onto a second display" failure the
preemption code was written to prevent, arriving from the other direction.

That is a real bug class, not a measurement artefact: it needs no misbehaving client,
just two connects close together, which is exactly what a multi-host switcher or a UI
that opens a preview while the main session is live will do.

### Refusal versus local client slowness

I do not think any of this was local CPU. The distinguishing evidence: at 4 probes,
three streams each sustained ~107-113 fps, so three concurrent software decodes fit
comfortably. At 5 probes the four non-survivors reported `frames=0` with a clean close,
not a degraded frame count - starvation produces few frames, not zero after a
successful Welcome. Zero frames plus a host-side display count that stays at two is
host-side session termination, not decode contention.

What I cannot yet attribute with certainty is the exact mechanism at 5 probes: whether
the later Hellos preempted the earlier ones once the first pipelines registered, or
whether two of the five simply never won a display. Both are consistent with the
journal. Either way the admission path is timing-dependent, which is the finding.

### Effect on the sequencing recommendation

The order does not change: capability 1 is still first, 3(a) second, 2 last and still
the trap. Two amendments:

1. **Add a small host-side fix before capability 1 ships.** Registration must happen
   before or atomically with the admission decision, or the live table must be keyed by
   identity rather than by session, so simultaneous same-identity connects resolve to
   one session instead of up to N. This is days, not weeks, and it is a prerequisite for
   the "warm" story: a warm session whose identity is not actually exclusive will
   collide with the switch back.
2. **Capability 2 must give the relay its own identity.** Section 2 assumed A is a
   client of B as an ordinary client. Given this race, a relay connection and A's own
   session on B can both be admitted and then fight over B's display. A dedicated relay
   identity per upstream makes the collision visible instead of accidental, and makes
   revocation meaningful.

### The mechanism, and the fix

The window is between **admission** and **insertion**, and it is wide enough to hold the
whole session bring-up. At `93f083d` the two halves are:

- `crates/slipstream-host/src/protocol/slipstream1/handshake.rs:141` calls
  `preempt_same_identity(peer_fp)` and `:155` calls `admit(peer_fp)` — both **read** the
  live-session table (`crates/ss-vdisplay/src/display/admission.rs:47-50`).
- `crates/slipstream-host/src/protocol/slipstream1/serve.rs:551` calls
  `admission::register(..)` — the only **write** — and it runs after the handshake, the
  Welcome, the Start, and (historically) the display pipeline coming up.

The table is keyed by session `id`, not by identity, so a Hello that has not registered
yet does not exist for the next Hello. It is not that the key is missing at admission
time — the certificate fingerprint is available at `handshake.rs:131` — it is that the
*entry* is: two Hellos in one burst both read zero rows, both get `Separate`, and each
builds its own output. That is exactly the observed 3-displays-for-one-client run.

### Current implementation: two-step native handshake

The live native path calls `preempt_same_identity(peer_fp)` and then `admit(..)` as two separate operations in `crates/slipstream-host/src/protocol/slipstream1/handshake.rs:130-145`; `preempt_same_identity` reads live rows, and `admit` pushes a new row later. `crates/ss-vdisplay/src/display/admission.rs` has the pure `decide` helper and the same-identity preemption helper, but no `claim`, `Claimed`, or `LiveGuard` entry point exists in this tree and the handshake does not use a guard. The earlier description of an atomic claim therefore described a proposed fix, not current code.

**Concurrency consequence.** The live table is keyed by session `id`, not by identity, so two Hellos in one burst can both read zero rows, both choose `Separate`, and each build its own output. The certificate fingerprint is available at `handshake.rs:131`; what is missing is an atomic operation that decides and inserts while holding the table lock. That is the observed 3-displays-for-one-client failure mode.

**Proposed fix (not yet implemented): one atomic claim.** `admission::claim(conflict, identity, mode, stop, label)` would take the table mutex once, compute `decide(..)` against rows already present, copy this identity's prior stop flags (reconnect zombies), and push this session's entry before releasing. It would return `Claimed { decision, own_zombies, guard }`; the native handshake would call it where `preempt_same_identity` + `admit` run now, before the Welcome, and `serve_session` would hold the guard exactly as long as the session.

Why this would be true by construction: a concurrent Hello either takes the mutex before this one and is visible to `decide`, or after and sees this claim. There is no interleaving in which two same-identity Hellos both observe an empty table. The critical section would remain a scan plus a push; the 1500 ms preemption grace, the Welcome, and pipeline setup happen after the lock is dropped.

**Preemption would be preserved, not disabled.** `own_zombies` would still be returned and signalled, and `Admission::Steal` would still collect victims' stop flags. The old `preempt_same_identity` + `admit` pair remains for the GameStream plane and tests.

**Failure path must be designed, not assumed.** A real guard would be the cleanup mechanism on every exit of `serve_session`, including a handshake that fails after the claim. A test such as `failed_claim_is_reclaimable` would be needed to pin that a dropped claim leaves no zombie. None of that exists today.

**Tests still to write** in `crates/ss-vdisplay/src/display/admission.rs` (`cargo test -p ss-vdisplay --lib admission`): `simultaneous_claims_admit_one_primary` (two threads, one primary, one zombie, table empty after guards drop) and `failed_claim_is_reclaimable`. A `pre_fix_ordering_admits_two_same_identity_hellos` test pins the old two-access ordering as documentation, not as a regression gate.

**Still racy: the GameStream plane.** `protocol/gamestream/stream.rs:272` still registers late, after its own admission at `protocol/gamestream/nvhttp.rs:176`. Two GameStream sessions from one identity can still race; the same fix would need to claim at admission and hold a guard for the launch, but it is out of scope here.

**Capability 1 depends on this fix.** The multi-host switcher holds a session per host and brings the previous one back warm; if a switch-away and a switch-back overlap, or a UI opens a preview while the main session is live, they are exactly the simultaneous-Hellos case. Without an atomic claim this manifests as duplicate virtual displays on a host the operator thought was idle, so the fix belongs ahead of capability 1 rather than behind it.

