//! T771: one-click local play. The launcher starts this machine's own realm (osfr-gateway and
//! osfr-login from a `server/` folder beside it), and PLAY joins it through the ordinary realm
//! login. The servers are child processes the launcher owns: dropping [`LocalRealm`] stops them.
//! No shell, no batch file, no Python. The server id is made once and kept, so the local profile
//! and its enrolled key stay valid across runs.

use std::{
    fs, io,
    net::{SocketAddr, TcpStream, UdpSocket},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

/// The AUTH2 broker port the launcher's realm login uses (`main.rs` `AUTH_BROKER_PORT`).
pub const BROKER_PORT: u16 = 20043;
/// The account handle the local profile is enrolled under.
pub const LOCAL_HANDLE: &str = "local";
/// How long a server may take to open its port before PLAY gives up and shows its log.
const START_TIMEOUT: Duration = Duration::from_secs(45);

/// Every path the local realm needs, explicit so nothing is guessed at launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSettings {
    /// osfr-gateway.exe, osfr-login.exe and osfr-account.exe.
    pub bin_dir: PathBuf,
    pub reference: PathBuf,
    pub content: PathBuf,
    /// The character save (kept between runs).
    pub save: PathBuf,
    /// The account/character identity file; its `.accounts.json` sibling is the account store.
    pub identity: PathBuf,
    /// The client login key (`references/rust-login/client-login-key.bin`).
    pub login_key: PathBuf,
    pub terrain: Option<PathBuf>,
    pub world: Option<PathBuf>,
    pub dev_tools: bool,
}

impl LocalSettings {
    /// The packaged layout: `<root>/server/{bin,reference,content,state,keys,terrain,world}`.
    pub fn packaged(root: &Path) -> Self {
        let server = root.join("server");
        let optional = |name: &str| Some(server.join(name)).filter(|path| path.is_dir());
        Self {
            bin_dir: server.join("bin"),
            reference: server.join("reference"),
            content: server.join("content"),
            save: server.join("state").join("character.json"),
            identity: server.join("state").join("identity.json"),
            login_key: server.join("keys").join("client-login-key.bin"),
            terrain: optional("terrain"),
            world: optional("world"),
            dev_tools: false,
        }
    }

    /// The account store osfr-login and the gateway share (osfr-login's default sibling).
    pub fn account_store(&self) -> PathBuf {
        self.identity.with_extension("accounts.json")
    }

    /// Required inputs that do not exist; PLAY refuses while any is missing.
    pub fn missing(&self) -> Vec<PathBuf> {
        let files = ["osfr-gateway.exe", "osfr-login.exe", "osfr-account.exe"]
            .map(|name| self.bin_dir.join(name));
        let mut missing: Vec<PathBuf> = files
            .into_iter()
            .chain([self.login_key.clone()])
            .filter(|path| !path.is_file())
            .collect();
        missing.extend(
            [&self.reference, &self.content]
                .into_iter()
                .filter(|dir| !dir.is_dir())
                .cloned(),
        );
        missing
    }
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
        "--reference".into(),
        path(&settings.reference),
        "--state".into(),
        path(&settings.save),
        "--seconds".into(),
        "0".into(),
        "--resources".into(),
        path(&client.join("Resources")),
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
    vec![
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
    ]
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

/// Enroll the local profile's public key with the bundled `osfr-account` (no console window).
pub fn enroll(settings: &LocalSettings, public_key_hex: &str) -> io::Result<()> {
    let mut command = Command::new(settings.bin_dir.join("osfr-account.exe"));
    command.args(enroll_args(settings, public_key_hex));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "osfr-account could not enroll the local profile ({status})"
        )))
    }
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
    pub login: SocketAddr,
    pub broker: SocketAddr,
    pub log_dir: PathBuf,
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
        let log_dir = realm_dir.join("logs");
        fs::create_dir_all(&log_dir)?;
        let handoff_key = realm_dir.join("handoff.key");
        let mut key = [0u8; 16];
        getrandom::getrandom(&mut key).map_err(|_| io::Error::other("no OS randomness"))?;
        fs::write(&handoff_key, key)?;
        let plan = RunPlan {
            gateway: free_udp_port()?,
            login: free_udp_port()?,
            broker: SocketAddr::from(([127, 0, 0, 1], BROKER_PORT)),
            server_id: realm_dir.join("server.id"),
            handoff_key,
            log_dir: log_dir.clone(),
        };
        server_identity(&plan.server_id)?;
        let mut realm = Self {
            children: Vec::new(),
            login: plan.login,
            broker: plan.broker,
            log_dir,
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
            login: "127.0.0.1:1".parse().unwrap(),
            broker: "127.0.0.1:1".parse().unwrap(),
            log_dir: PathBuf::new(),
        };
        drop(realm);
        assert_ne!(id, 0);
    }
}
