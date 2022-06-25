/*!
A wireless network chooser.

For proper function, this definitely needs wpa_supplicant running with the
the correct config file.

`# wpa_supplicant -B -i <IFACE> -c <WPA_CONF>`

where `<IFACE>` is the `interface = ` option from your dmxwifi.toml config file,
and `<WPA_CONF>` is the `wpa_conf = ` from same.

It is also helpful to set a sudoers rule to allow the user to run `dhclient`
without a password. If you have `group = "netdev"` set in your configuration
file, then a sudoers stanza like

`%netdev     ALL = NOPASSWD: /usr/sbin/dhclient`

If that isn't an option, you can always make entering the sudo password a
little more reliable with a GUI askpass program, like ssh-askpass. This
involves a setting in your /etc/sudo.conf:

`Path askpass /path/to/your/askpass/program`

NOTE: see https://superuser.com/a/1719355/1704665 if you are having
inexplicable syntax errors with this line.

And if that isn't feasable for some reason, the last option is setting
the `askpass =` option in the `dmxwifi.toml` config file.

*/
use std::collections::HashMap;
use std::fmt::Write as FmtWrite;
use std::io::{BufRead, BufReader, Write as IoWrite};
use std::path::Path;
use std::process::{Command, Stdio};

use camino::{Utf8PathBuf};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use dm_x::{Dmx, Item};

/// The library of saved access points gets serialized/deserialized from/to this.
type Library= HashMap<String, WapCfg>;

const USAGE: &str = "
usage: dmxwifi [ OPTION ] [ ARG ]

where OPTION can be
    -p, --password      set selected network password to ARG
    -f, --forget        forget selected network
";

/// Regex for parsing the ouput of "wpa_cli scan".
/// needs .multi_line(true).
const SCAN_RE: &str = r#"^([0-9a-f:]+)\t(\d+)\t(-?\d+)\t[^\t]+\t(.*)$"#;
/// Regex for parsing the output of "wpa_cli list_networks".
/// needs .multi_line(true).
const LIST_RE: &str = r#"^(\d+)\t[^\t]*\t([0-9a-f:]+)"#;
/// Regex for extracting passphrase from output of "wpa_passphrase".
const PASS_RE: &str = r#"\spsk=([0-9a-f]+)"#;

/// This gets deserialized from the configuration .toml file.
#[derive(Deserialize)]
struct ConfigFile {
    interface: Option<String>,
    library: Option<String>,
    wpa_socket: Option<String>,
    wpa_cli: Option<String>,
    dhclient: Option<String>,
    wpa_conf: Option<String>,
    askpass: Option<String>,
    group: Option<String>
}

impl ConfigFile {
    /// Attempt to deserialize a `ConfigFile` from a file at the given path.
    ///
    /// This is expected to fail in a lot of cases, so it just returns `None`
    /// on any errors.
    fn from_file<P: AsRef<Path>>(path: P) -> Option<ConfigFile> {
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => { return None; },
        };
        
        match toml::from_slice(&bytes) {
            Ok(cfgf) => Some(cfgf),
            Err(_) => None,
        }
    }
}

/**
Global variables and configuration options.

This gets generated from a combination of `default()` data and data from
a deserialized `ConfigFile`.
*/
struct Config {
    /// Name of the wireless interface to use. Default is `wlan0`.
    interface: String,
    /// Library of saved wireless networks and passwords.
    /// Default is `your_config_directory/dmxwifi_lib.toml`.
    library: Utf8PathBuf,
    /// Socket `wpa_cli` should use to communicate with `wpa_supplicant`.
    /// Default is `/var/run/wpa_supplicant`.
    wpa_socket: Utf8PathBuf,
    /// Path to the `wpa_cli` binary. Default is `/usr/sbin/wpa_cli`.
    wpa_cli: Utf8PathBuf,
    /// Path to the `dhclient` binary. Default is `/usr/sbin/dhclient`.
    dhclient: Utf8PathBuf,
    /// Path to use as the `wpa_supplicant` configuration file.
    /// Default is `your_config_directory/dmxwifi_wpa.conf`.
    wpa_conf: Utf8PathBuf,
    /// Path to "askpass" binary to use. Default is `None`, but suggested
    /// value is something like `/usr/bin/ssh-askpass` if you have it
    /// installed (and you are encouraged to install it if you don't).
    askpass: Option<Utf8PathBuf>,
    /// Group to set in `wpa_supplicant` configuration file that's allowed
    /// to use `wpa_cli`. You should set this to a group you're in. Default
    /// is `netdev`.
    group: String,
    /// `dmenu` configuration to use. This is set automatically from the
    /// system settings, probably `your_config_directory/dmx.toml`.
    dmx: Dmx,
}

impl Default for Config {
    fn default() -> Self {
        let mut library = dmxtools::config_directory().unwrap();
        library.push("dmxwifi_lib.toml");
        let mut wpa_conf = dmxtools::config_directory().unwrap();
        wpa_conf.push("dmxwifi_wpa.conf");
        
        Self {
            interface: "wlan0".to_owned(),
            library,
            wpa_socket: Utf8PathBuf::from("/var/run/wpa_supplicant"),
            wpa_cli: Utf8PathBuf::from("/usr/sbin/wpa_cli"),
            dhclient: Utf8PathBuf::from("/usr/sbin/dhclient"),
            wpa_conf,
            askpass: None,
            group: "netdev".to_owned(),
            dmx: Dmx::automagiconf(),
        }
    }
}

impl Config {
    /// Return a `Config::default()` with any options appearing in
    /// `cfgf` overriding the defaults.
    fn from_config_file(cfgf: ConfigFile) -> Config {
        let mut cfg = Config::default();
        
        if let Some(iface) = cfgf.interface {
            cfg.interface = iface;
        }
        if let Some(path) = cfgf.library {
            cfg.library = Utf8PathBuf::from(path);
        }
        if let Some(path) = cfgf.wpa_socket {
            cfg.wpa_socket = Utf8PathBuf::from(path);
        }
        if let Some(path) = cfgf.wpa_cli {
            cfg.wpa_cli = Utf8PathBuf::from(path);
        }
        if let Some(path) = cfgf.dhclient {
            cfg.dhclient = Utf8PathBuf::from(path);
        }
        if let Some(path) = cfgf.wpa_conf {
            cfg.wpa_conf = Utf8PathBuf::from(path);
        }
        if let Some(path) = cfgf.askpass {
            cfg.askpass = Some(Utf8PathBuf::from(path));
        }
        if let Some(group) = cfgf.group {
            cfg.group = group;
        }
        
        cfg
    }
    
    /**
    Attempt to configure from the usual places, in this order:
      * `$DMXWIFI_CONFIG` environment variable
      * `$XDG_CONFIG_HOME/dmxwifi.toml`
      * `$HOME/.config/dmxwifi.toml`
      * from a `Config::default()` (always works)
    */
    fn new() -> Config {
        if let Ok(path) = std::env::var("DMXWIFI_CONFIG") {
            if let Some(cfgf) = ConfigFile::from_file(path) {
                return Config::from_config_file(cfgf);
            }
        }
        
        if let Ok(mut path) = dmxtools::config_directory() {
            path.push("dmxwifi.toml");
            if let Some(cfgf) = ConfigFile::from_file(&path) {
                return Config::from_config_file(cfgf);
            }
        }
        
        Config::default()
    }
    
    /// Return a base `Command` for running `wpa_cli` with the interface
    /// and socket arguments set.
    fn wpa_cli_cmd(&self) -> Command {
        let mut cmd = Command::new(&self.wpa_cli);
        cmd.args(["-i", &self.interface, "-p", &self.wpa_socket.as_str()]);
        
        cmd
    }
    
    /// Return the output from running `wpa_cli` with the given arguments.
    ///
    /// These are in addition to the base arguments set by
    /// `Config::wpa_cli_cmd()`.
    fn wpa_cli_output(&self, args: &[&str]) -> Result<String, String> {
        let out_bytes = self.wpa_cli_cmd()
            .args(args)
            .output()
            .map_err(|e| format!(
                "Error invoking wpa_cli w/args {:?}: {}",
                args, &e
            ))?
            .stdout;
        String::from_utf8(out_bytes)
            .map_err(|e| format!(
                "Output from wpa_cli w/args {:?} is not UTF-8: {}",
                args, &e
            ))
    }
}

/// A wireless network configuration/password saved in the `Library`.
#[derive(Debug, Serialize, Deserialize)]
struct WapCfg {
    /// MAC address of the access point.
    mac: String,
    /// ESSID of the wireless network.
    essid: String,
    /// Plaintext password saved for this network.
    pwd: String,
    /// Password encrypted in "pre-shared key" form (as output by
    /// `wpa_passphrase`.
    psk: String,
}

impl Item for &WapCfg {
    fn key_len(&self) -> usize {
        self.essid.chars().count()
    }
    
    fn line(&self, key_len: usize) -> Vec<u8> {
        format!("{:<width$}  {}", &self.essid, &self.mac, width = key_len)
            .into_bytes()
    }
}

impl WapCfg {
    /// Generate a `wpa_supplicant` configuration file `network=` stanza
    /// for this wireless network.
    fn to_wpa_conf_stanza(&self) -> String {
        format!(
            "network={{\n\tbssid={}\n\tssid=\"{}\"\n\t#psk=\"{}\"\n\tpsk={}\n}}\n",
            &self.mac, &self.essid, &self.pwd, &self.psk
        )
    }
}

/// A wireless network detected by scanning (possibly combined with saved
/// password info from the library).
#[derive(Debug)]
struct Wap {
    /// MAC address presented by physical device.
    mac: String,
    /// Channel frequency (MHz)
    freq: String,
    /// Signal strength (in dBm)
    level: String,
    /// Wireless network "name".
    essid: String,
    /// Saved network name (if this network is saved to the library and the
    /// currently scanned name is different from the saved name).
    old_essid: Option<String>,
    /// Saved newtwork password (if this networks is saved in the library).
    pwd: Option<String>,
    /// Saved PSK (if this network is saved in the libraray).
    psk: Option<String>,
}

impl Wap {
    /// Add any saved information from the Library about this nework.
    fn get_psk(mut self, lib: &Library) -> Wap {
        if let Some(wapcfg) = lib.get(&self.mac) {
            self.pwd = Some(wapcfg.pwd.clone());
            self.psk = Some(wapcfg.psk.clone());
            if &self.essid != &wapcfg.essid {
                self.old_essid = Some(wapcfg.essid.clone())
            }
        }
        return self
    }
    
    /// Given a password, generate a `WapCfg` library entry from this
    /// scan result.
    fn into_cfg(self,  pwd: &str) -> Result<WapCfg, String> {
        let wpa_out = Command::new("wpa_passphrase")
            .args([&self.essid, pwd])
            .output()
            .map_err(|e| format!("Error invoking wpa_passphrase: {}", &e))?
            .stdout;
        let wpa_out = String::from_utf8(wpa_out)
            .map_err(|e| format!("wpa_passphrase output not UTF-8: {}", &e))?;
        
        let passphrase_pattern = Regex::new(PASS_RE).unwrap();
        
        match passphrase_pattern.captures(&wpa_out) {
            None => Err("Unable to match output of wpa_passphrase.".to_owned()),
            Some(m) => {
                let w = WapCfg {
                    mac: self.mac,
                    essid: self.essid,
                    pwd: pwd.to_owned(),
                    psk: m[1].to_owned(),
                };
                Ok(w)
            },
        }
    }
}

impl Item for Wap {
    fn key_len(&self) -> usize {
        self.essid.chars().count()
    }
    
    fn line(&self, key_len: usize) -> Vec<u8> {
        let config_char = match &self.psk {
            Some(_) => '*',
            None => ' ',
        };
        
        let mut line = format!(
            "{} {:<width$} {:>4} dBm  {:>4}  {}",
            config_char, &self.essid, &self.level, &self.freq, &self.mac,
            width = key_len
        );
        if let Some(id) = &self.old_essid {
            line.push(' ');
            line.push_str(id);
        }
        line.into_bytes()
    }
}

/// End the program, printing the given message to stderr.
fn die(code: i32, message: &str) -> ! {
    eprintln!("{}", &message);
    std::process::exit(code);
}

/// Attempt to deserialize the `Library` of saved networks at the given `path`.
fn load_library<P: AsRef<Path>>(path: P) -> Result<Library, String> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)
        .map_err(|e| format!(
            "Error reading known access points from \"{}\": {}",
            path.display(), &e
        ))?;
    let map: Library = toml::from_slice(&bytes)
        .map_err(|e| format!(
            "Error deserializing known access points from \"{}\": {}",
            path.display(), &e
        ))?;
    Ok(map)
}

/// Save the `Library` file at the given `path`.
fn save_library<P: AsRef<Path>>(path: P, lib: &Library) -> Result<(), String> {
    let lib_text = toml::to_string_pretty(lib)
        .map_err(|e| format!("Error serializing library file: {}", &e))?;
    
    let path = path.as_ref();
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(path)
        .map_err(|e| {
            format!(
                "Unable to open library file \"{}\" for create/truncate/write: {}",
                path.display(), &e
            )
        })?;
    
    f.write_all(&lib_text.as_bytes())
        .map_err(|e| format!(
            "Error writing to library file \"{}\": {}",
            path.display(), &e
        ))
}

/// Save the library in a format that `wpa_supplicant` can read as a
/// configuration file.
fn save_wpa_config(cfg: &Config, lib: &Library) -> Result<(), String> {
    let mut buff = format!(
"update_config=1
ctrl_interface=DIR={} GROUP={}
",
        &cfg.wpa_socket, &cfg.group
    );
    
    for (_, wcfg) in lib.iter() {
        let stanza = wcfg.to_wpa_conf_stanza();
        if let Err(e) = write!(&mut buff, "{}", &stanza) {
            eprintln!("Error generating wpa_supplicant configuration file: {}", &e);
        }
    }
    
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(&cfg.wpa_conf)
        .map_err(|e| {
            format!(
                "Unable to open wpa_supplicant config file \"{}\" for create/truncate/write: {}",
                &cfg.wpa_conf, &e
            )
        })?;
    
    f.write_all(&buff.as_bytes())
        .map_err(|e| format!(
            "Error writing wpa_supplicant config file \"{}\": {}",
            &cfg.wpa_conf, &e
        ))
}

/// Scan all wireless networks in range; cross-reference these with and add
/// any data from the saved `Library`.
fn scan(cfg: &Config, lib: &Library) -> Result<Vec<Wap>, String> {
    let mut wpa_cli = cfg.wpa_cli_cmd()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Unable to execute \"{}\": {}", &cfg.wpa_cli, &e))?;

    let mut stdin = wpa_cli.stdin.take().unwrap();
    let mut stdout = BufReader::new(wpa_cli.stdout.take().unwrap());

    stdin.write_all(b"scan\n")
        .map_err(|e| format!("Error writing to wpa_cli subprocess: {}", &e))?;
    let mut buff = String::with_capacity(64);
    loop {
        match stdout.read_line(&mut buff) {
            Ok(0) => {
                let estr = "End of wpa_cli output unexpected.".to_owned();
                return Err(estr);
            },
            Err(e) => {
                let estr = format!("Error reading wpa_cli output: {}", &e);
                return Err(estr);
            }
            _ => { /* this is okaysauce */}
        }
    
        if buff.contains("CTRL-EVENT-SCAN-RESULTS") {
            break;
        } else if buff.contains("CTRL-EVENT-SCAN-FAILED") {
            return Err("wpa_cli scan failed.".to_owned());
        }
        buff.clear();
        
    }
    stdin.write_all(b"quit\n")
        .map_err(|e| format!("Error writing to wpa_cli subprocess: {}", &e))?;
    
    let _ = wpa_cli.wait()
        .map_err(|e| format!("Error awaiting wpa_cli subprocess: {}", &e))?;
    
    let scan_results = cfg.wpa_cli_output(&["scan_results"])?;
    
    let scan_pattern = RegexBuilder::new(SCAN_RE)
        .multi_line(true)
        .build()
        .unwrap();
    
    let mut waps: Vec<Wap> = scan_pattern.captures_iter(&scan_results)
        .map(|m| Wap {
            mac: m[1].to_owned(),
            freq: m[2].to_owned(),
            level: m[3].to_owned(),
            essid: m[4].to_owned(),
            old_essid: None,
            pwd: None,
            psk: None,
        }.get_psk(lib))
        .collect();
    
    waps.sort_by_cached_key(|x| -x.level.parse::<i32>().unwrap());
    Ok(waps)
}

/**
Request that the user select a network in range and associate the given
password with it.

Also re-save the `Library` with this new information and write (and instruct
the daemon to reload) a new `wpa_supplicant` configuration.
*/
fn set_password(cfg: &Config, pwd: &str) -> Result<(), String> {
    let mut lib = load_library(&cfg.library).unwrap_or(Library::new());
    let mut v = scan(&cfg, &lib)?;
    let n = match cfg.dmx.select("", &v)? {
        Some(n) => n,
        None => { return Ok(()); },
    };
    let wcfg = v.swap_remove(n).into_cfg(pwd)?;
    
    lib.insert(wcfg.mac.clone(), wcfg);
    save_library(&cfg.library, &lib)?;
    save_wpa_config(cfg, &lib)?;
    reconfigure(cfg)
}

/// Request that the user select a network and then remove it from the library.
///
/// Resave the library and `wpa_supplicant` configuration data.
fn forget_network(cfg: &Config) -> Result<(), String> {
    let mut lib = load_library(&cfg.library)?;
    
    let mac = {
        let mut v: Vec<&WapCfg> = lib.values().collect();
        v.sort_unstable_by(|a, b| a.essid.cmp(&b.essid));
    
        match cfg.dmx.select("", &v)? {
            Some(n) => v[n].mac.clone(),
            None => { return Ok(()); },
        }
    };
    
    let _ = lib.remove(&mac);
    save_library(&cfg.library, &lib)?;
    save_wpa_config(cfg, &lib)
}

/// Invoke `wpa_cli` to request that the `wpa_supplicant` daemon reload its
/// configuration file.
fn reconfigure(cfg: &Config) -> Result<(), String> {
    match cfg.wpa_cli_cmd()
        .arg("reconfigure")
        .status()
    {
        Ok(_) => Ok(()),
        Err(e) => Err(format!(
            "Error reconfiguring wpa_supplicant: {}", &e
        )),
    }
}

/// Request the user select from a list of detectable networks, and attempt
/// to connect to it.
fn connect(cfg: &Config) -> Result<(), String> {
    let lib = match load_library(&cfg.library) {
        Err(s) => {
            eprintln!("{}", &s);
            Library::new()
        },
        Ok(lib) => lib,
    };
    
    let wapz = scan(cfg, &lib)?;
    let wap = match cfg.dmx.select("", &wapz).unwrap() {
        Some(n) => &wapz[n],
        None => { return Ok(()); },
    };
    
    let list_out = cfg.wpa_cli_output(&["list_networks"])?;
    
    let list_pattern = RegexBuilder::new(LIST_RE)
        .multi_line(true)
        .build()
        .unwrap();
    
    for m in list_pattern.captures_iter(&list_out) {
        if &wap.mac == &m[2] {
            let wap_n = &m[1];
            let _ = cfg.wpa_cli_cmd().args(["select_network", wap_n]).status()
                .map_err(|e| format!(
                    "Error invoking wpa_cli to select_network {}: {}",
                    wap_n, &e
                ))?;
                
            let mut dhclient_cmd = Command::new("sudo");
            dhclient_cmd.args(["-A", &cfg.dhclient.as_str()]);
            if let Some(askpass) = &cfg.askpass {
                dhclient_cmd.env("SUDO_ASKPASS", askpass.as_str());
            }
            return match dhclient_cmd .status() {
                Ok(_) => Ok(()),
                Err(e) => Err(format!(
                    "Error invoking {} as root: {}",
                    &cfg.dhclient, &e
                )),
            };
        }
    }
        
    Err("Selected network not configured.".to_owned())
}

fn main() {
    let cfg = Config::new();
    
    // This has pretty simple argument semantics, so we don't use `clap`
    // or anything.
    let args: Vec<String> = std::env::args().collect();
    let action = args.get(1);
    let arg = args.get(2);
    
    match action.map(String::as_str) {
        Some("-p") | Some("--password") => {
            if let Some(p) = arg {
                if let Err(e) = set_password(&cfg, p.as_str()) {
                    die(1, &e);
                }
            } else {
                die(2, &format!("{} option requires password.", &action.unwrap()));
            }
        },
        Some("-f") | Some("--forget") => {
            if let Err(e) = forget_network(&cfg) {
                die(1, &e);
            }
        },
        Some(opt) => {
            die(2, &format!("Unknown option: {}\n{}", &opt, USAGE));
        }
        None => {
            if let Err(e) = connect(&cfg) {
                die(1, &e);
            }
        },
    }
}