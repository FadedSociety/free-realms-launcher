//! T771: one-click local play. The launcher starts this machine's own realm (osfr-gateway and
//! osfr-login from a `server/` folder beside it), and PLAY joins it through the ordinary realm
//! login. The servers are child processes the launcher owns: dropping [`LocalRealm`] stops them.
//! No shell, no batch file, no Python. The server id is made once and kept, so the local profile
//! and its enrolled key stay valid across runs.

use std::{
    fs, io,
    net::{IpAddr, SocketAddr, TcpStream, UdpSocket},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

/// The AUTH2 broker port the launcher's realm login uses (`main.rs` `AUTH_BROKER_PORT`).
pub const BROKER_PORT: u16 = 20043;
/// L070: a hosted realm's fixed game and login ports, so friends (and a router) know them.
pub const GATEWAY_PORT: u16 = 20041;
pub const LOGIN_PORT: u16 = 20042;
/// The account handle the local profile is enrolled under.
pub const LOCAL_HANDLE: &str = "local";
/// How long a server may take to open its port before PLAY gives up and shows its log.
const START_TIMEOUT: Duration = Duration::from_secs(45);
/// SHA-256 of the client's 16-byte login protocol key. Every Free Realms client holds the key in
/// FreeRealms.exe as a NUL-terminated base64 string; the launcher copies it from the player's own
/// client on first local play, so no release ever ships it.
pub const LOGIN_KEY_SHA256: &str =
    "61af8e29bc4e31ee0d642b55aca16c14fe0e25c8996cbab7ee6e558c64fa28f1";

/// L024/T575: the community character-creator overlay a stock client needs (osfr-login pins the
/// two customization tables; the stock starting-set table is empty). Client path and SHA-256 of
/// each file, the list `scripts/bundle/client_overlay.py` pins. The launcher fetches them from the
/// community's public host, the source every community-launcher install uses, so no release of
/// ours carries them (README "Client overlay for the character creator").
pub const CREATOR_OVERLAY: [(&str, &str); 22] = [
    (
        "Resources/CharacterCreate/CharacterCreateItemStartingSets.txt",
        "89cc60e8979179e9af77aada3f3e8b64042757b249ee92745c361ce40b153c24",
    ),
    (
        "Resources/CharacterCreateCustomizationSkinTone.txt",
        "485d20174e96c53021d9644d3ba943f0a016fb0413d1be8fa8f728bea528dd9d",
    ),
    (
        "Resources/CharacterCreateCustomizationFaceType.txt",
        "9d4d7acd63c7926ed3d4c03f99116e929061eef4ae25e1a2b6208c738f2c8525",
    ),
    (
        "custom/sg_dressing_room.gzne",
        "bf49bec0f03aefc6d1bf14fc41049023b31f408efc0345da3f67595c3133a3c6",
    ),
    (
        "custom/sg_dressing_room_0_0.gck2",
        "19da101cd06d820983156c5c257550d3105f7e06784d30cf512e7798d4b62626",
    ),
    (
        "custom/sg_dressing_room_0_0.gcnk",
        "b914368be071c1ad1b50af6558fd6b05e09d01c08d76b278e3e14fc59e6c0aee",
    ),
    (
        "custom/sg_dressing_room_0_4.gck2",
        "5a84441f17e19390ab727fe3291080a7b428fc22b742520fc98615751e65fbba",
    ),
    (
        "custom/sg_dressing_room_0_4.gcnk",
        "81b8b0ec435fb90b0b24e4b90f54c2b5f48996ab07c5b784143a1de283a6aa83",
    ),
    (
        "custom/sg_dressing_room_0_8.gck2",
        "366c78da9bd4c2d663b09fce714398ff499497036b2c0399ef9e4f54213d2ac2",
    ),
    (
        "custom/sg_dressing_room_0_8.gcnk",
        "7a1a36c1ae9ca701eb67a469ad40f3e40f1bdc1af4d29141f80370f740a70537",
    ),
    (
        "custom/sg_dressing_room_4_0.gck2",
        "0da5ddf394aaec8be9c7f5a8d94b772c446aa001e32309fc40a8356525626d4c",
    ),
    (
        "custom/sg_dressing_room_4_0.gcnk",
        "4aa545ca68323f23a7fe9576720021edd30b9e9f83c5424fa07992c19d635291",
    ),
    (
        "custom/sg_dressing_room_4_4.gck2",
        "9860b4e2515c515053ccaaeefe32df308ceb40f1b74e94038c7d377f3c1dc342",
    ),
    (
        "custom/sg_dressing_room_4_4.gcnk",
        "c58f065fe9cb7a2664730cbf4253a5478c27f0087422b2d7aa182ad89b401530",
    ),
    (
        "custom/sg_dressing_room_4_8.gck2",
        "5a66e6629dda786f80434bac864580e5926fc5ddcb8f31fb3a4d090359340977",
    ),
    (
        "custom/sg_dressing_room_4_8.gcnk",
        "35647418448753fab50a365ec7811bdb2e978b66bc79f41eb0ce64ccba371b5a",
    ),
    (
        "custom/sg_dressing_room_8_0.gck2",
        "785717a9d939e8200f014d03cd05b785ca4cc078fe7812228a2b05433f2cd2ff",
    ),
    (
        "custom/sg_dressing_room_8_0.gcnk",
        "17eac9a5f01b9f8fb4b0adbe298a8217d32e8ad28975793ae4b957f99f973181",
    ),
    (
        "custom/sg_dressing_room_8_4.gck2",
        "9bdd369b4e2e9dd14a2c4bbd843f2b78f21f79c443c82ee18f1a3534ab9dd5ef",
    ),
    (
        "custom/sg_dressing_room_8_4.gcnk",
        "8769d331e649240c5ceed613c5430892a833bad1082dfcddc83ee2c3e129eda4",
    ),
    (
        "custom/sg_dressing_room_8_8.gck2",
        "efc6f6167180cef6976e64e0024932157b8e175fb8b2bc5598ade139702893e7",
    ),
    (
        "custom/sg_dressing_room_8_8.gcnk",
        "adb407d869408fc37bef6a9283d27c331ab92c9e3114f9146b04fbd67f99b06e",
    ),
];
const OVERLAY_HOST: &str = "https://opensourcefreerealms.com/client/";
/// Larger than any overlay file (the biggest scene chunk is a few MB).
const OVERLAY_FILE_LIMIT: u64 = 64 << 20;

/// Install every overlay file the client copy lacks or holds in another version, each checked
/// against its pin before it is written. Files already matching are left alone, so this costs
/// one hash per file after the first run.
pub fn ensure_creator_overlay(client: &Path) -> io::Result<()> {
    let cancel = std::sync::atomic::AtomicBool::new(false);
    for (path, pin) in CREATOR_OVERLAY {
        let target = client.join(path);
        if fs::read(&target).is_ok_and(|bytes| crate::patch::sha256(&bytes).is_ok_and(|h| h == pin))
        {
            continue;
        }
        let url =
            crate::patch::Url::parse(&format!("{OVERLAY_HOST}{path}")).map_err(io::Error::other)?;
        let bytes = crate::patch::fetch_bytes(&url, OVERLAY_FILE_LIMIT, &cancel)
            .map_err(|e| io::Error::other(format!("Could not download {path}: {e}")))?;
        if crate::patch::sha256(&bytes).map_err(io::Error::other)? != pin {
            return Err(io::Error::other(format!(
                "{path} from the community host does not match its pin"
            )));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let staged = target.with_extension("overlay-tmp");
        fs::write(&staged, &bytes)?;
        fs::rename(&staged, &target)?;
    }
    Ok(())
}

/// Every path the local realm needs, explicit so nothing is guessed at launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSettings {
    /// osfr-gateway.exe, osfr-login.exe and osfr-account.exe.
    pub bin_dir: PathBuf,
    /// Gameplay content; the gateway builds world entry from its `startup/` folder.
    pub content: PathBuf,
    /// The character save (kept between runs).
    pub save: PathBuf,
    /// The account/character identity file; its `.accounts.json` sibling is the account store.
    pub identity: PathBuf,
    /// The client login key, copied from the player's FreeRealms.exe ([`ensure_login_key`]).
    pub login_key: PathBuf,
    pub terrain: Option<PathBuf>,
    pub world: Option<PathBuf>,
    pub dev_tools: bool,
    /// L070: this PC's LAN or VPN address to host friends on. `None` keeps the realm on this PC.
    /// A hosted realm is open: a friend's first login makes their account, unless the
    /// `whitelist.txt` beside the account store lists the handles that may join.
    pub host: Option<IpAddr>,
}

impl LocalSettings {
    /// The packaged layout: `<root>/server/{bin,content,state,keys,terrain,world}`.
    pub fn packaged(root: &Path) -> Self {
        let server = root.join("server");
        let optional = |name: &str| Some(server.join(name)).filter(|path| path.is_dir());
        Self {
            bin_dir: server.join("bin"),
            content: server.join("content"),
            save: server.join("state").join("character.json"),
            identity: server.join("state").join("identity.json"),
            login_key: server.join("keys").join("client-login-key.bin"),
            terrain: optional("terrain"),
            world: optional("world"),
            dev_tools: false,
            host: None,
        }
    }

    /// The account store osfr-login and the gateway share (osfr-login's default sibling).
    pub fn account_store(&self) -> PathBuf {
        self.identity.with_extension("accounts.json")
    }

    /// L070: the handles allowed to join a hosted realm, one per line; absent = open to anyone.
    pub fn whitelist(&self) -> PathBuf {
        self.identity.with_file_name("whitelist.txt")
    }

    /// Required server inputs that do not exist; PLAY refuses while any is missing. The login
    /// key is not one: [`LocalRealm::start`] takes it from the client.
    pub fn missing(&self) -> Vec<PathBuf> {
        let files = ["osfr-gateway.exe", "osfr-login.exe", "osfr-account.exe"]
            .map(|name| self.bin_dir.join(name));
        let mut missing: Vec<PathBuf> = files.into_iter().filter(|path| !path.is_file()).collect();
        if !self.content.is_dir() {
            missing.push(self.content.clone());
        }
        missing
    }
}

/// Write `path` from the client's FreeRealms.exe when it is missing (first local play).
pub fn ensure_login_key(path: &Path, client: &Path) -> io::Result<()> {
    if path.is_file() {
        return Ok(());
    }
    let exe = fs::read(client.join("FreeRealms.exe"))?;
    let key = find_login_key(&exe, LOGIN_KEY_SHA256).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "FreeRealms.exe does not hold the Free Realms login key; choose the original client folder",
        )
    })?;
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| io::Error::other("no key folder"))?,
    )?;
    fs::write(path, key)
}

/// The NUL-terminated 24-character base64 string (16 bytes, `==` padded) in `exe` whose bytes
/// hash to `sha256_hex`.
pub fn find_login_key(exe: &[u8], sha256_hex: &str) -> Option<[u8; 16]> {
    exe.windows(26).find_map(|w| {
        if w[0] != 0 || w[25] != 0 || &w[23..25] != b"==" {
            return None;
        }
        let key = base64_16(&w[1..23])?;
        (crate::patch::sha256(&key).ok()? == sha256_hex).then_some(key)
    })
}

/// 22 base64 characters (the `==` already stripped) -> 16 bytes.
fn base64_16(text: &[u8]) -> Option<[u8; 16]> {
    let (mut bits, mut count, mut out) = (0u32, 0, Vec::with_capacity(16));
    for &c in text {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        bits = bits << 6 | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    out.try_into().ok()
}

/// One run's endpoints and secret files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunPlan {
    pub gateway: SocketAddr,
    pub login: SocketAddr,
    pub broker: SocketAddr,
    pub server_id: PathBuf,
    pub handoff_key: PathBuf,
    pub log_dir: PathBuf,
}

/// The gateway command line (the native harness's, without its relays or a time limit).
pub fn gateway_args(settings: &LocalSettings, client: &Path, plan: &RunPlan) -> Vec<String> {
    let path = |p: &Path| p.display().to_string();
    let mut args = vec![
        plan.gateway.to_string(),
        "--state".into(),
        path(&settings.save),
        "--seconds".into(),
        "0".into(),
        "--resources".into(),
        path(&client.join("Resources")),
        // L072: the referee panel names items from the player's own client text.
        "--client-locale".into(),
        path(&client.join("locale")),
        "--content".into(),
        path(&settings.content),
        "--identity".into(),
        path(&settings.identity),
        "--account-store".into(),
        path(&settings.account_store()),
        "--handoff-key-file".into(),
        path(&plan.handoff_key),
    ];
    if let Some(terrain) = &settings.terrain {
        args.extend(["--terrain".into(), path(terrain)]);
    }
    if settings.host.is_some() {
        args.push("--allow-lan".into());
    }
    if settings.dev_tools {
        args.extend([
            "--dev-tools".into(),
            "--client-logs".into(),
            path(&client.join("Logs")),
        ]);
        if let Some(world) = &settings.world {
            args.extend(["--world".into(), path(world)]);
        }
    }
    args
}

/// The login command line in key-pair mode: the launcher's own profile key is the credential.
pub fn login_args(settings: &LocalSettings, client: &Path, plan: &RunPlan) -> Vec<String> {
    let path = |p: &Path| p.display().to_string();
    let mut args = vec![
        "--bind".into(),
        plan.login.to_string(),
        "--auth-bind".into(),
        plan.broker.to_string(),
        "--gateway".into(),
        plan.gateway.to_string(),
        "--resources".into(),
        path(&client.join("Resources")),
        "--identity".into(),
        path(&settings.identity),
        "--key-file".into(),
        path(&settings.login_key),
        "--state".into(),
        path(&settings.save),
        "--handoff-key-file".into(),
        path(&plan.handoff_key),
        "--account-store".into(),
        path(&settings.account_store()),
        "--server-id-file".into(),
        path(&plan.server_id),
        "--seconds".into(),
        "0".into(),
    ];
    if settings.host.is_some() {
        args.push("--allow-lan".into());
        let whitelist = settings.whitelist();
        if whitelist.is_file() {
            args.extend(["--enroll-handles".into(), path(&whitelist)]);
        } else {
            args.push("--open-enrollment".into());
        }
    }
    args
}

/// `osfr-account create`: enroll the local profile's public key once.
pub fn enroll_args(settings: &LocalSettings, public_key_hex: &str) -> Vec<String> {
    vec![
        "create".into(),
        "--store".into(),
        settings.account_store().display().to_string(),
        "--handle".into(),
        LOCAL_HANDLE.into(),
        "--public-key".into(),
        public_key_hex.into(),
    ]
}

/// Enroll the local profile's public key with the bundled `osfr-account` (no console window);
/// returns the new account's id.
pub fn enroll(settings: &LocalSettings, public_key_hex: &str) -> io::Result<u64> {
    if let Some(parent) = settings.account_store().parent() {
        fs::create_dir_all(parent)?;
    }
    let out = account_tool(settings, &enroll_args(settings, public_key_hex))?;
    out.trim()
        .strip_prefix("created account ")
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| io::Error::other(format!("osfr-account answered {out:?}")))
}

/// L070: the host's own account is the realm's admin (the role behind `realm` commands); friends
/// who join are players. Run before the servers start, like [`enroll`].
pub fn make_admin(settings: &LocalSettings, account_id: u64) -> io::Result<()> {
    let store = settings.account_store().display().to_string();
    let id = account_id.to_string();
    account_tool(
        settings,
        &[
            "set-role",
            "--store",
            &store,
            "--account-id",
            &id,
            "--role",
            "admin",
        ]
        .map(String::from),
    )
    .map(drop)
}

fn account_tool(settings: &LocalSettings, args: &[String]) -> io::Result<String> {
    let mut command = Command::new(settings.bin_dir.join("osfr-account.exe"));
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command.output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(io::Error::other(format!(
            "osfr-account {} failed ({}): {}",
            args[0],
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// L070: this PC's addresses a realm can be hosted on: the LAN route's and, when a VPN such as
/// Tailscale is up, its 100.64.0.0/10 address. A UDP connect only picks the route the OS would
/// use; no packet is sent. Only private addresses are offered (the servers refuse public ones).
pub fn host_addresses() -> Vec<IpAddr> {
    let mut found = Vec::new();
    for probe in ["192.0.2.1:9", "100.100.100.100:9"] {
        let route = UdpSocket::bind("0.0.0.0:0").and_then(|socket| {
            socket.connect(probe)?;
            socket.local_addr()
        });
        if let Ok(address) = route {
            let ip = address.ip();
            let private = match ip {
                IpAddr::V4(v4) => {
                    let [a, b, ..] = v4.octets();
                    v4.is_private() || (a == 100 && (64..128).contains(&b))
                }
                IpAddr::V6(_) => false,
            };
            if private && !found.contains(&ip) {
                found.push(ip);
            }
        }
    }
    found
}

/// The kept 32-byte server id (made on first run); its hex is the local realm's identity.
pub fn server_identity(path: &Path) -> io::Result<[u8; 32]> {
    if let Ok(bytes) = fs::read(path) {
        return bytes.try_into().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "local server id is not 32 bytes",
            )
        });
    }
    let mut id = [0u8; 32];
    getrandom::getrandom(&mut id).map_err(|_| io::Error::other("no OS randomness"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, id)?;
    Ok(id)
}

/// A free loopback UDP port (R: released before the server binds it; a lost race fails the start
/// with the server's own message).
fn free_udp_port() -> io::Result<SocketAddr> {
    UdpSocket::bind("127.0.0.1:0")?.local_addr()
}

/// The running local realm. Dropping it stops both servers.
pub struct LocalRealm {
    children: Vec<Child>,
    /// L071: the client uploads its portrait to the gateway's port.
    pub gateway: SocketAddr,
    pub login: SocketAddr,
    pub broker: SocketAddr,
    pub log_dir: PathBuf,
    /// The server identity friends compare with what their launcher shows.
    pub identity: [u8; 32],
}

impl LocalRealm {
    /// Start the gateway, then login, in `realm_dir` (the profile's local-realm folder), and wait
    /// until both listen. A failure stops whatever started and names the log to read.
    pub fn start(settings: &LocalSettings, client: &Path, realm_dir: &Path) -> io::Result<Self> {
        if let Some(missing) = settings.missing().first() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("The local realm is missing {}", missing.display()),
            ));
        }
        ensure_login_key(&settings.login_key, client)?;
        let log_dir = realm_dir.join("logs");
        fs::create_dir_all(&log_dir)?;
        let handoff_key = realm_dir.join("handoff.key");
        let mut key = [0u8; 16];
        getrandom::getrandom(&mut key).map_err(|_| io::Error::other("no OS randomness"))?;
        fs::write(&handoff_key, key)?;
        let (gateway, login, broker) = match settings.host {
            Some(ip) => (
                SocketAddr::new(ip, GATEWAY_PORT),
                SocketAddr::new(ip, LOGIN_PORT),
                SocketAddr::new(ip, BROKER_PORT),
            ),
            None => (
                free_udp_port()?,
                free_udp_port()?,
                SocketAddr::from(([127, 0, 0, 1], BROKER_PORT)),
            ),
        };
        let plan = RunPlan {
            gateway,
            login,
            broker,
            server_id: realm_dir.join("server.id"),
            handoff_key,
            log_dir: log_dir.clone(),
        };
        let identity = server_identity(&plan.server_id)?;
        let mut realm = Self {
            children: Vec::new(),
            gateway: plan.gateway,
            login: plan.login,
            broker: plan.broker,
            log_dir,
            identity,
        };
        let gateway = gateway_args(settings, client, &plan);
        realm.spawn(
            &settings.bin_dir.join("osfr-gateway.exe"),
            &gateway,
            "gateway",
        )?;
        realm.wait(|| udp_taken(plan.gateway), "gateway")?;
        let login = login_args(settings, client, &plan);
        realm.spawn(&settings.bin_dir.join("osfr-login.exe"), &login, "login")?;
        realm.wait(
            || TcpStream::connect_timeout(&plan.broker, Duration::from_millis(200)).is_ok(),
            "login",
        )?;
        Ok(realm)
    }

    fn spawn(&mut self, exe: &Path, args: &[String], name: &str) -> io::Result<()> {
        let out = fs::File::create(self.log_dir.join(format!("{name}.log")))?;
        let err = out.try_clone()?;
        let mut command = Command::new(exe);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        self.children.push(command.spawn()?);
        Ok(())
    }

    fn wait(&mut self, ready: impl Fn() -> bool, name: &str) -> io::Result<()> {
        let started = Instant::now();
        while started.elapsed() < START_TIMEOUT {
            if ready() {
                return Ok(());
            }
            if let Some(child) = self.children.last_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(io::Error::other(format!(
                        "The local {name} stopped ({status}); see {}",
                        self.log_dir.join(format!("{name}.log")).display()
                    )));
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "The local {name} did not start; see {}",
                self.log_dir.join(format!("{name}.log")).display()
            ),
        ))
    }

    /// Whether every server is still running.
    pub fn running(&mut self) -> bool {
        self.children
            .iter_mut()
            .all(|child| matches!(child.try_wait(), Ok(None)))
    }
}

/// A UDP port is taken once the server has bound it.
fn udp_taken(address: SocketAddr) -> bool {
    UdpSocket::bind(address).is_err()
}

impl Drop for LocalRealm {
    fn drop(&mut self) {
        // Only the handles this launcher started; never a process found by name.
        for child in self.children.iter_mut().rev() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> LocalSettings {
        LocalSettings::packaged(Path::new(r"C:\play"))
    }

    fn plan() -> RunPlan {
        RunPlan {
            gateway: "127.0.0.1:50001".parse().unwrap(),
            login: "127.0.0.1:50002".parse().unwrap(),
            broker: "127.0.0.1:20043".parse().unwrap(),
            server_id: PathBuf::from(r"C:\realm\server.id"),
            handoff_key: PathBuf::from(r"C:\realm\handoff.key"),
            log_dir: PathBuf::from(r"C:\realm\logs"),
        }
    }

    #[test]
    fn the_packaged_layout_and_command_lines_match_the_native_harness() {
        let s = settings();
        assert_eq!(
            s.account_store(),
            PathBuf::from(r"C:\play\server\state\identity.accounts.json")
        );
        let client = Path::new(r"C:\client");
        let gateway = gateway_args(&s, client, &plan());
        assert_eq!(gateway[0], "127.0.0.1:50001");
        let pairs: Vec<_> = gateway[1..]
            .chunks(2)
            .map(|p| (p[0].as_str(), p[1].as_str()))
            .collect();
        assert!(pairs.contains(&("--seconds", "0")), "no time limit");
        assert!(pairs.contains(&("--resources", r"C:\client\Resources")));
        assert!(pairs.contains(&("--client-locale", r"C:\client\locale")));
        assert!(pairs.contains(&("--handoff-key-file", r"C:\realm\handoff.key")));
        assert!(!gateway.contains(&"--dev-tools".to_owned()));
        let login = login_args(&s, client, &plan());
        let pairs: Vec<_> = login
            .chunks(2)
            .map(|p| (p[0].as_str(), p[1].as_str()))
            .collect();
        for expected in [
            ("--bind", "127.0.0.1:50002"),
            ("--auth-bind", "127.0.0.1:20043"),
            ("--gateway", "127.0.0.1:50001"),
            ("--server-id-file", r"C:\realm\server.id"),
            (
                "--account-store",
                r"C:\play\server\state\identity.accounts.json",
            ),
            ("--seconds", "0"),
        ] {
            assert!(pairs.contains(&expected), "{expected:?}");
        }
        assert_eq!(
            enroll_args(&s, "ab"),
            [
                "create",
                "--store",
                r"C:\play\server\state\identity.accounts.json",
                "--handle",
                "local",
                "--public-key",
                "ab"
            ]
        );
    }

    /// L070: hosting binds the LAN or VPN address and opens the realm to new players, unless the
    /// whitelist beside the account store names who may join; plain local play adds neither.
    #[test]
    fn hosting_opens_the_realm_on_the_lan_unless_a_whitelist_names_the_players() {
        let dir = std::env::temp_dir().join(format!("osfr-host-{}", std::process::id()));
        let mut s = LocalSettings::packaged(&dir);
        let client = Path::new(r"C:\client");
        let has = |args: &[String], flag: &str| args.iter().any(|arg| arg == flag);
        for args in [
            gateway_args(&s, client, &plan()),
            login_args(&s, client, &plan()),
        ] {
            assert!(!has(&args, "--allow-lan") && !has(&args, "--open-enrollment"));
        }
        s.host = Some("192.168.1.5".parse().unwrap());
        assert!(has(&gateway_args(&s, client, &plan()), "--allow-lan"));
        let login = login_args(&s, client, &plan());
        assert!(has(&login, "--allow-lan") && has(&login, "--open-enrollment"));
        fs::create_dir_all(s.whitelist().parent().unwrap()).unwrap();
        fs::write(s.whitelist(), "friend_a\n").unwrap();
        let login = login_args(&s, client, &plan());
        assert!(!has(&login, "--open-enrollment"));
        let list = s.whitelist().display().to_string();
        assert!(
            login
                .windows(2)
                .any(|w| w == ["--enroll-handles", list.as_str()])
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dev_tools_add_the_harness_flags() {
        let mut s = settings();
        s.dev_tools = true;
        let args = gateway_args(&s, Path::new(r"C:\client"), &plan());
        assert!(
            args.windows(2)
                .any(|w| w == ["--client-logs", r"C:\client\Logs"])
        );
    }

    #[test]
    fn a_missing_server_refuses_to_start_and_names_it() {
        let dir = std::env::temp_dir().join(format!("osfr-local-{}", std::process::id()));
        let s = LocalSettings::packaged(&dir);
        assert!(!s.missing().is_empty());
        let error = LocalRealm::start(&s, &dir, &dir.join("realm"))
            .err()
            .unwrap();
        assert!(error.to_string().contains("missing"), "{error}");
    }

    #[test]
    fn the_server_id_is_made_once_and_kept() {
        let dir = std::env::temp_dir().join(format!("osfr-local-id-{}", std::process::id()));
        let path = dir.join("server.id");
        let first = server_identity(&path).unwrap();
        assert_eq!(server_identity(&path).unwrap(), first);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dropping_the_realm_stops_its_children() {
        #[cfg(windows)]
        let child = Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
            .spawn()
            .unwrap();
        #[cfg(not(windows))]
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let id = child.id();
        let realm = LocalRealm {
            children: vec![child],
            gateway: "127.0.0.1:1".parse().unwrap(),
            login: "127.0.0.1:1".parse().unwrap(),
            broker: "127.0.0.1:1".parse().unwrap(),
            log_dir: PathBuf::new(),
            identity: [0; 32],
        };
        drop(realm);
        assert_ne!(id, 0);
    }

    #[test]
    fn the_login_key_comes_from_the_clients_exe_and_only_when_missing() {
        let key: [u8; 16] = std::array::from_fn(|i| i as u8);
        let hash = crate::patch::sha256(&key).unwrap();
        // A decoy base64 string with another key comes first; only the pinned hash matches.
        let mut exe = b"MZ junk QUFBQUFBQUFBQUFBQUFBQQ== more".to_vec();
        exe.extend_from_slice(b" AAECAwQFBgcICQoLDA0ODw== tail");
        assert_eq!(find_login_key(&exe, &hash), Some(key));
        assert_eq!(find_login_key(&exe, &"0".repeat(64)), None);
        assert_eq!(
            find_login_key(b" AAECAwQFBgcICQoLDA0ODw==", &hash),
            None,
            "unterminated"
        );

        let root = std::env::temp_dir().join(format!("osfr-login-key-{}", std::process::id()));
        let (client, path) = (
            root.join("client"),
            root.join("server/keys/client-login-key.bin"),
        );
        fs::create_dir_all(&client).unwrap();
        assert!(
            ensure_login_key(&path, &client).is_err(),
            "no FreeRealms.exe"
        );
        fs::write(client.join("FreeRealms.exe"), b"no key here").unwrap();
        assert!(
            ensure_login_key(&path, &client).is_err(),
            "not the Free Realms client"
        );
        assert!(!path.exists());
        // The pinned hash is the real client's, so a planted file stands in for a found key.
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, key).unwrap();
        ensure_login_key(&path, &client).unwrap();
        assert_eq!(fs::read(&path).unwrap(), key, "an existing key is kept");
        let _ = fs::remove_dir_all(&root);
    }
}
