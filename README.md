# Free Realms Expanded launcher

Standalone Rust launcher. It prepares an existing, writable client copy and
starts its local `FreeRealms.exe`. Choose that copy and an optional locale pair
in Settings. A realm's pinned patch manifest can install or repair files in that
copy. It never executes a batch file or runs Python.

The current W98 skin follows design direction A: deep-blue glass panels,
raised bumpy frames, glossy green controls, attached tabs and an imagegen forest
backdrop. Four orange vines grow from the banner wordmark on launch. Art sources,
hashes and slice boxes are listed in `assets/ui/README.md`; review captures and
the readability audit are in `lab/reports/rust-launcher-w98/`.

To join a friend's realm, open **Settings → Join a realm** and enter the game
address. Paste the 64-digit server identity the host sends, or press **Ask the
realm for its identity**, then check the short fingerprint against the one the
host sees. Pick an account handle and choose **Create my key**. The launcher
stores the private key and trusted profile in your Windows user profile, adds
the realm to the list, and shows the public key. A realm hosted with **Host for
friends** makes your account on your first PLAY; otherwise send that public key
to the host for enrollment. It is safe to share; never send a private key.
Return to **Realms** and choose the saved account beside PLAY.

The trusted server identity is the one you saved: pasted from the host, or
fetched once and checked by its fingerprint. Login refuses any other identity.
Login goes to the realm's AUTH2 broker: by default the game address's IP at port
20043, and only a loopback or private/VPN address (`default_broker` in
`src/main.rs`, `broker_ip_allowed` in `src/auth.rs`). The host must enroll the
public key for the supplied account handle. A refused login explains that the
host may still need to add the key. The broker intentionally returns one
generic refusal for unknown, disabled, wrong-key and locked accounts.

The launcher keeps the command-line key tools for operator setup and recovery:

```powershell
& $exe --new-key my-realm
& $exe --print-public-key my-realm
```

`--new-key` writes an owner-only key under the current user's
`APPDATA\FreeRealmsExpanded\profiles\<id>` directory and prints only the
public key. `--print-public-key` exports the public half for an existing local
profile. These commands run before UI startup and return their output directly.
The controller option controls this launcher window. It does not inject game
input or start the existing in-game Python bridge. Settings are session-only.

PLAY also runs an asset responder on `127.0.0.1` for as long as the client
runs, so keep the launcher open while you play. Without one the client exits
with code 27 when `manifest.crc` fails (`notes/Research/Client Runtime
Dependencies.md`). Like the native harness, it answers `/manifest.crc` with the
CRC-32 and size of the client's own `Assets_manifest.txt`, so the client keeps
that local manifest, and answers `/<digits>/<name>` with the file of exactly
that name in the client's `indirect-assets/` folder, byte for byte: a `.z` file
must already be the A1B2C3D4 envelope the client inflates. A realm's patch
manifest fills that folder. Every other request, including `..`, separators,
`%` escapes, absolute paths and links (hard links too), gets 404.

## Play locally

The launcher can run its own realm on your PC, so no host is needed. The server files live in a `server/`
folder beside `osfr-launcher.exe`, and the launcher gets and updates them itself from the latest
[GitHub Release of free-realms-expanded](https://github.com/FadedSociety/free-realms-expanded/releases):

- **First use.** Until `server/` is complete the button reads **Play locally (downloads the server first)**.
  Pressing it downloads the latest release into `server/`, checks every file, installs it and stops. Nothing is
  started for you: press **Play locally (no host needed)** afterwards.
- **Each start.** With a server installed, the launcher checks GitHub in the background. If a newer release
  exists the message line says "A server update is available", then shows the download progress, and only the
  files that changed are downloaded. The window stays usable, and the launcher never starts the server or the
  client by itself. Play locally waits while a check or download is running.
- **Offline.** If GitHub cannot be reached, the message line says so and the installed server is used as it is.
- **Repair.** Delete `server/.osfr-release.json` to make the next start re-check every file against the release.

```text
server/
  bin/        osfr-gateway.exe, osfr-login.exe, osfr-account.exe
  content/    gameplay content (data/content from free-realms-expanded)
  state/      character.json and identity.json, your saves (created and kept here)
  keys/       client-login-key.bin   (copied from your FreeRealms.exe on first local play)
  terrain/    optional
  world/      optional
```

When that folder is complete, **Play locally (no host needed)** starts the gateway and login servers with no
console windows, makes this PC's key-pair profile for the local realm the first time (and enrolls its public key
with `osfr-account`), then launches your client against it. The servers stop when the client closes; your saves
stay in `server/state`. Logs are in the local profile's `realm/logs` folder under
`%APPDATA%\FreeRealmsExpanded\profiles`.

### Host for friends

Tick **Host for friends (LAN or VPN)** before **Play locally** to run the realm on this PC's LAN address, or
its Tailscale (100.64.0.0/10) address when one is up, on ports 20041 (game, UDP), 20042 (login, UDP) and 20043
(key-pair login, TCP). Windows asks once to let osfr-gateway and osfr-login through its firewall: allow private
networks. While hosting, the launcher shows the address and the server fingerprint, and **Copy join details**
copies both for your friends. The realm is open: a friend's first login makes their player account, at most
three new accounts an hour from one address and 64 in all. To let only certain players in, list their handles,
one per line, in `server/state/whitelist.txt`. Your own account is the realm's admin. Public internet addresses
are refused; friends outside your network join through a VPN such as Tailscale.

### No client yet?

**Settings → Download the client** fetches the community's client files (about 0.9 GB, 1,788 files) from the
community host's `clientmanifest.xml` into `client/` beside the launcher, the same source the community launcher
uses. That manifest leaves out the ten `AssetsW_*.pack` world packs; whether a client made from it loads every
zone is not yet tested, so keep your own complete client folder if you have one.
It checks every file against the manifest's XXH64 hash, resumes an interrupted download, and points the client
folder setting there when done. Our releases never carry client files. If PLAY reports that DirectX is missing
(`d3dx9_31.dll`), the link under the message opens Microsoft's DirectX End-User Runtime page.

### What the release contains

The project lead decides what a release contains; the launcher installs exactly the files the release manifest
lists and nothing else. A release carries one manifest asset, `server-manifest.json`, and one asset per file:

```json
{
  "version": 1,
  "tag": "v0.1.0",
  "files": [
    { "path": "bin/osfr-gateway.exe", "size": 12345678, "sha256": "<64 hex digits>", "asset": "osfr-gateway.exe" }
  ]
}
```

- `version` is the manifest format, currently `1`. `tag` must equal the release's own tag, so a manifest cannot
  be replayed from another release.
- `path` is relative to `server/`, with `/` separators: portable ASCII names only, no `..`, drive letters, UNC
  or absolute paths, Windows device names, alternate data streams, or leading dots. `state/` is refused
  outright.
- `size` and `sha256` are checked for every download. `asset` is the file's name on the release (letters,
  digits, `.`, `_`, `-`). GitHub allows at most 1000 assets per release.
- Caps: a 4 MiB manifest, 5000 files, 2 GiB per file and 4 GiB in total.

### How an update stays safe

- **HTTPS only.** The launcher talks to `api.github.com` and `github.com` and follows the redirect to GitHub's
  release storage (`objects.githubusercontent.com` or `release-assets.githubusercontent.com`), re-checking every
  hop. Anything else, and plain HTTP, is refused (literal loopback HTTP exists only for the automated tests).
  TLS is Windows' own WinHTTP, so there is no extra TLS library in the launcher.
- **Stage, verify, then swap.** Files are downloaded into `server/.update-stage/` and each is checked for size
  and SHA-256 before anything in `server/` changes. Only when all of them pass are the files swapped in, with
  the replaced ones set aside and put back if any step fails. A crash mid-swap is rolled back on the next
  start. The release marker `server/.osfr-release.json` is written last, so a half-finished update is never
  recorded as installed. Nothing downloaded is run before it is verified.
- **Your data is yours.** `server/state/` is never read or written by an update. Files under `keys/` and any
  `*.key` or `*.pem` file are installed only when missing and never overwritten. Files you add yourself are
  left alone; files an older release shipped and a newer one dropped are removed.
- **Trust.** The manifest comes from the release itself, so trust rests on TLS to GitHub and on who can publish
  releases in the repository. There is no signature on the manifest yet.

## Building

On Windows, with Rust (rustup) and an unpacked [llvm-mingw](https://github.com/mstorsjo/llvm-mingw) UCRT
toolchain:

```powershell
rustup target add x86_64-pc-windows-gnullvm
$env:LLVM_MINGW_ROOT = 'C:\tools\llvm-mingw-ucrt-x86_64'
./build.ps1 build
./build.ps1 test
& ./target/x86_64-pc-windows-gnullvm/debug/osfr-launcher.exe
```

`build.ps1` runs `cargo` (from `CARGO_HOME` when set, else from PATH) with `--locked` for the
`x86_64-pc-windows-gnullvm` target, linking with llvm-mingw's clang. Pass `-LlvmMingw <folder>` instead of
`LLVM_MINGW_ROOT` if you prefer. Output goes to `target/` unless `CARGO_TARGET_DIR` is set. Add `-Release` for
a release build. The real-broker test in `tests/auth.rs` runs only when `OSFR_SERVER_BIN` names a folder holding
the server's `osfr-login.exe`; otherwise it is skipped.

The target receives Rust's `+crt-static`: the failing old artifact had empty
rustflags and imported the missing `libunwind.dll`. Never add llvm-mingw's bin
folder to PATH: host build scripts then select an incompatible compiler.
Start the executable from PowerShell or cmd; Git Bash's loader is unsuitable
for this UCRT build. The launcher keeps its OpenGL renderer.

Controller: left stick/D-pad moves focus, A selects, B returns, LB advances,
Start toggles Settings. Tab/Shift+Tab, Space/Enter, Escape and mouse also work.
The original `scripts/launcher/gamepad.json` is compiled in, not duplicated.

Capture the running launcher without any game launch (output folder must exist;
Play is disabled for the entire capture run, including controller activation):

```powershell
$reports = Join-Path $lab 'reports/rust-launcher-20261015'
New-Item -ItemType Directory -Force $reports | Out-Null
New-Item -ItemType Directory -Force (Join-Path $lab 'build/w86b') | Out-Null
$env:OSFR_LAUNCHER_LOG = Join-Path $lab 'build/w86b/home-review.log'
& $exe --screenshot (Join-Path $reports 'launcher.png')
$env:OSFR_LAUNCHER_LOG = Join-Path $lab 'build/w86b/settings-review.log'
& $exe --settings-page --screenshot (Join-Path $reports 'settings.png')
Remove-Item Env:OSFR_LAUNCHER_LOG
```

The screenshot comes from the native window's egui renderer and the app then
closes. `--settings-page` can also open Settings without capture mode. Existing
output files are refused. Capture failure, timeout, invalid arguments and
`run_native` errors return a nonzero exit code.

W86c adds `--window-size WIDTHxHEIGHT` for bounded layout review, using logical
egui points (640–3840 by 480–2160); default remains 940 x 790. At this PC's 150%
display scaling, `--window-size 853x533` captures 1280 x 800 physical pixels;
`--window-size 1280x800` captures 1920 x 1200. Short windows use a smaller banner
and tighter spacing. The standard single-realm Home and Settings fit at
1280 x 800 physical pixels without scrolling; longer realm lists may scroll. Captures remain
unable to prepare or launch the game at any size.

The skin bundles Lilita One for titles and captions, keeps Ubuntu body text and
readable address fonts, and uses egui's real controls for keyboard/controller
focus, Space/Enter activation and accessibility. Font and art provenance are in
DEPENDENCIES.md. W86c's final screenshots and before/after audit are under
`lab/reports/rust-launcher-20261016/`; `notes/Design/Rust Launcher.md` records the
13-test result and focus-ring canary. Physical controller actions remain open.

`OSFR_LAUNCHER_LOG` enables an exclusive, bounded 1 MiB file of startup/rendering
diagnostics and panic/error output. Its parent must exist and the file must be
unused. It does not log launch arguments, tokens or form contents. Omit the
variable for normal operation.

**W86b evidence (2026-10-16):** offline build and 12 tests pass. Home and
Settings each saved a 1410 x 1185 PNG through their own running-window capture.
Physical controller navigation, actual client launch, release packaging and
second-PC acceptance remain unverified. See `notes/Design/Rust Launcher.md` for
root-cause logs, provenance, security review and the remaining full-launcher gates.

## Evidence (2026-10-22)

Native runs are under `lab/runs/`. Every client run cited here used the
`run-local-baseline.py` harness, not this launcher.

| What | State | Evidence |
|---|---|---|
| Launcher window, Settings and join screen draw | Proven (window captures) | `lab/reports/rust-launcher-20261016/`, `lab/reports/rust-launcher-w131/` |
| Key creation and AUTH2 broker login against a real `osfr-login` | Proven by tests only | `tests/auth.rs` `real_broker_enrol_generated_key_and_receive_token`; pin canary `wrong_pin_sends_no_proof_canary` |
| Pinned patch download, resume and repair | Proven by loopback tests only; no real patch host | `tests/patch.rs`; `lab/reports/rust-launcher-w121/README.md` |
| Launcher starts `FreeRealms.exe` | Pending (lead native run, L024) | — |
| The client needs a `manifest.crc` asset server | Proven natively (harness) | `client-baseline-20261008T015537Z` exited with code 27 after four failed `manifest.crc` requests; `client-baseline-20261021T193008Z` got 7 of 339 asset requests answered 200 (`manifest.crc`, fence, lamp and sconce models). Since T442 the launcher's loopback responder serves `manifest.crc` and indirect assets: `IndirectServerAddress` points at it (`assets::Responder`, `src/lib.rs`; `src/tests.rs:121-122`), and only the client's other URLs still go to `127.0.0.1:9` (`offline_urls`). Not yet run natively (L024) |
| Dev-mode login, Character Select, world entry | Proven natively (harness) | `client-baseline-20261020T173403Z`, `client-baseline-20261020T204059Z` |
| Account-store login: legacy save migrated into the store, then world entry | Proven natively (harness) | `client-baseline-20261014T023623Z` (`notes/Reviews/Native Login Accounts 023623.md`) |
| Role-checked `realm` commands without `--dev-tools` on an account-store gateway (admin `realm god` ran, player refused) | Proven natively (harness, dev-mode login) | `client-baseline-20261021T193008Z` |
| Gateway HMAC handoff | Proven natively (harness, dev-mode login) | L023 removed the public fixed ticket on account-store servers: login gives every account, 1/1 included, an HMAC binding ticket (`prototype/osfr-tools/src/login.rs:457-459`), and the gateway refuses any other ticket and logs `auth_failure` (`prototype/osfr-tools/src/bin/osfr-gateway.rs:1133-1139`). In `client-baseline-20261022T005637Z` the real client (1/1) entered on it through an account-store gateway. Tickets are still reusable and never expire (Security Model S02/L3) |
| Key-pair login of the real client (broker token redeemed by the client) | Not run | The harness runs login in loopback dev mode; key-pair native verification belongs with this launcher (`lab/autopilot/native-notes/2026-10-20.md`, harness repairs) |
| Character creation on a fresh key-pair account | Not run | `notes/Reviews/Native Login Accounts 023623.md` (Open) |
| VPN broker | Proven with `osfr-bot` only | L053 ran gateway, login and broker on the host's Tailscale address and passed the bot from the host and from Windows Sandbox (through the host's NAT, not Tailscale) (`lab/autopilot/native-notes/2026-10-22.md`, `lab/runs/lan-test-20261022T041757Z`). A second PC and the real client are still not run |
| Physical controller, release package, fresh second PC | Not run | — |

## W98 skin evidence (2026-10-16)

W98 replaces the earlier geometry-only button and panel surfaces with bundled
imagegen sprites. The nine-slice renderer holds corners fixed, tiles the bumpy
edge strips, and stretches only the plain panel centre. The capture-only options
`--test-focus-play` and `--test-vine-growth-ms 0..1500` exist to produce review
images; screenshot mode disables Play and closes after capture. Build and test
results, including any host-specific test limitations, are recorded in
`lab/reports/rust-launcher-w98/README.md`.

## W112 art and login evidence (2026-10-16)

W112 moves vine growth to the banner's right edge and checks every revealed
pixel against protected text and starburst rectangles. It gives the list row,
text field and status plank smooth bevels, aligns repeating button/panel edges
to complete bead periods, and draws a cover-fitted backdrop over the full
viewport. PLAY uses the selected realm's stored profile when its profile ID is
set and passes the resulting session token through the existing launch path.
The W112 report contains before/after captures, per-gate test output, art
provenance and remaining limitations.

## W121 pinned patching (2026-10-16)

Settings applies to the selected realm. Set the existing client folder, the
full HTTPS manifest URL, its trusted SHA-256 pin, and optionally a mirror
manifest URL. Obtain the pin independently from the realm operator. Never use
a hash supplied by the same untrusted download as the trust anchor. Pins,
URLs and realm choices are session-only, like the existing launcher settings.
There is no default download host or bundled community-client distribution.

Normal patch checks start automatically once the folder, URL and pin are set.
**Verify and repair** repeats the same check and rehashes existing files;
same-size tampering is detected. PLAY stays disabled for configured patches
until that exact folder/realm/URL/pin selection verifies successfully. With no
patch URL, the existing local-client workflow remains available. File count,
bytes and current file appear on both pages. Cancel preserves resumable data;
closing the launcher signals cancellation too.

The version-1 JSON manifest has this shape (the example digest is SHA-256 of
an empty file):

```json
{"version":1,"files":[{"path":"data/example.bin","size":0,"sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}]}
```

File URLs are relative to the manifest's directory. A mirror serves the exact
same manifest and relative file tree. Serve Content-Length on every response;
resumed responses must have an exact `206 Content-Range`, or return the whole
file with `200` to restart. Redirects, content compression, URL credentials,
queries and fragments are rejected. HTTPS uses Windows certificate validation
and TLS 1.2; plain HTTP is accepted only for literal loopback test servers.
No cookies, system credentials or login tokens enter patch requests.

Downloads go to `<file>.osfr-part` alongside their destination. A valid complete
partial can be installed without another download. Interrupted partials resume
with Range; a bad final hash discards their contents and retries from zero.
Three attempts use 100/200/400 ms backoff and try the optional mirror. Only a
size/hash-verified, synced partial replaces the installed file, using a
same-directory atomic rename. Existing files survive failed verification.

Manifest limits (R): 4 MiB, 50,000 files, 16 GiB per file, 128 GiB total.
Paths use portable ASCII components, at most 220 bytes, with `/` separators;
traversal, Windows device names/ADS, duplicate case aliases, overlapping paths,
launcher-state names, junctions, symlinks and hard-linked targets/partials are
rejected. Use a private, user-owned client folder: hostile simultaneous local
filesystem mutation is outside the existing launcher's isolation boundary.
Do not run two patchers on the same copy. Atomicity is per file, not across
the whole manifest; a failed update must finish before PLAY unlocks.

Cancellation is checked between hash/I/O blocks and every 20 ms in backoff.
Synchronous WinHTTP calls have 3-second stage timeouts; cancellation waits for
an in-flight call to return. Manifest and file body attempts also have 30-second
and 120-second deadlines. A very large/slow file may require another resume.

W121 uses std, existing serde, Windows WinHTTP and Windows CNG SHA-256. No Cargo
dependency or lockfile change; `getrandom` stays at 0.2.17. It also centres field
text, increases panel inner padding, fits both standard compact pages, and
replaces raw profile IDs with the saved-profile dropdown and Local login.

`--test-patch-progress --screenshot <unused.png>` renders synthetic progress for
visual review only. It cannot download, prepare or launch anything and is
rejected without screenshot mode. Actual HTTP transfer/resume and the UI's
worker/PLAY route are separately tested. Full output, canary, threat model,
security self-review and before/final captures are in
`lab/reports/rust-launcher-w121/README.md`. Native game launch, public HTTPS host,
fresh-PC acceptance and normal-token stored-profile tests remain lead gates.

W131 adds the key-pair setup flow to Settings. `--test-join-flow --screenshot
<unused.png>` captures a synthetic example at any supported `--window-size`; it
does not create a key, save a profile, or contact a server. The default window
is 940 x 790 logical points. At 150% display scale, use `--window-size 853x533`
for a 1280 x 800 physical capture.

## Client overlay for the character creator (L024, private realms only)

A stock client's creator offers looks whose textures are missing or unproven
(skin tones 7/8 and four extra faces), has no starting sets, and its dressing-room
scene lacks nine terrain chunks. `scripts/bundle/client_overlay.py` lists the 22
community files that fix this, each with its SHA-256 pin and the path the
client opens; the native harness installs from that same list. `make_bundle.py`
builds it into the playtest bundle's `client-overlay/` folder, a W121 patch
folder; the host serves only that folder at the patch URL. On its own:

```powershell
python -I scripts/bundle/client_overlay.py --out <new folder>
```

Both refuse unless every cached source matches its pin (no overlay folder, and
no bundle), then print the manifest pin to give friends with the patch URL. The
bundle also puts the two pinned customization tables in its `login-resources/`,
so `osfr-login` refuses the looks they hide (its `listening` line shows
`"creator_allow_list":true`).
`client_overlay_lands_byte_identical` in `tests/patch.rs` (ignored; needs the lab
cache) installs it through W121.

**Provenance.** All 22 files were downloaded on 2026-10-08 from
`https://opensourcefreerealms.com/client/<overlay path>`. Each is listed in that
site's public `clientmanifest.xml` (SHA-256
`9555ac7c5361d1734334df99f03f0f2149b35ee579098ffb709c0ec91fd78b42`), which the
community [Launcher](https://github.com/Open-Source-Free-Realms/Launcher)
(`e6a2674`) installs from; length and XXHash64 match that manifest. Per-file URL,
size and hashes: `lab/references/asset-cache/cache-index.json` and
`creator-public-tables.json`.

| Overlay path | What it is |
| --- | --- |
| `Resources/CharacterCreate/CharacterCreateItemStartingSets.txt` | Community-populated table, 4 rows. The stock file has none, so creation fails (`startingSetId=-1`). |
| `Resources/CharacterCreateCustomizationSkinTone.txt` | The original table minus the 8 rows that select skin 7/8. |
| `Resources/CharacterCreateCustomizationFaceType.txt` | The original table minus 4 extra face rows. |
| `custom/sg_dressing_room.gzne`, `custom/sg_dressing_room_{0,4,8}_{0,4,8}.gcnk` and `.gck2` (19 files) | Community-made creator scene. Not original assets: they do not match the original manifest rows. |

**Licence: not stated.** The Launcher's AGPL-3.0 covers its code, not these
files, and the three tables derive from the original client's own. Lead
decision (2026-10-22): private realms (friends over a VPN, the Playtest Host
Runbook setup) may serve the overlay. These are the same files every
community-launcher user downloads publicly from opensourcefreerealms.com, so
pinned redistribution to invited friends follows the project's direction to use
community renditions of original content. A public release still waits until
the licence is clear.

**Local play (2026-10-28).** The launcher does not ship these files either: before it starts the
local realm it downloads any of the 22 the player's client copy lacks straight from that same
public community host (`CREATOR_OVERLAY` in `src/local.rs`) and writes each only after its SHA-256
matches the pin, exactly as a community-launcher install would.

**Client paths** (FreeRealms.exe). StartingSets: `./Resources/CharacterCreate/`
(0x017e802c, set by 0x0071efb0) + the file name (0x017e8dc8, read by 0x00723e90).
Customization tables: `Resources/` (0x017ed7d8, passed by 0x008ec05d to
0x00982f10) + `CharacterCreateCustomization` + the category suffix (0x0097f570) +
`.txt`; not `CharacterCreate/`, where the harness used to copy them. `custom/` is
an ordinary folder the client's loose-file index scans (notes/Research/Native
Character Selection Labels.md).

## Names in character select

The stock character select movie (`characterSelectV2.gfx`, Assets_006.pack entry
6072) titles each row with the character's id, so players see "1" and "0" instead
of their name and "Add a Character". Before each launch the launcher reads that
movie from the player's own client, checks its SHA-256, changes the one title
operand from `_id` to `_name` and writes the result to `custom/characterSelectV2.gfx`
(`src/character_select.rs`). No client file is shipped; an existing file there, or
a different client's movie, is left alone.

## Text the stock locale lacks

Some server text has no row in the stock locale; the opening's reconstructed
objective labels showed as `<STRING 0 NOT FOUND>` in the journal. The server
release lists those rows (hash, tag, text) in `content/locale-additions.json`.
Before each launch the launcher adds any missing row to the player's own
`locale/en_us_data.dat` and `.dir` (`src/client_text.rs`), keeping both in hash
order with the `.dir` header's `Count` and `MD5Checksum` updated: byte for byte
what the community locale tool (FreeRealmsLocaleTools, MIT) writes. Existing rows
are never changed.

## Licence

GNU Affero General Public License v3 or later (`LICENSE`), like the Free Realms Expanded server, since 2026-10-29
(it was MIT OR Apache-2.0 before). Anything built from this launcher must stay open under the same licence.
