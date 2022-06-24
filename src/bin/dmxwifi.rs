/*!
A wireless network chooser.

May need some

`# ip link set <interface> up`
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

/// Regex for parsing the ouput of "wpa_cli scan".
/// needs .multi_line(true).
const SCAN_RE: &str = r#"^([0-9a-f:]+)\t(\d+)\t(-?\d+)\t[^\t]+\t(.*)$"#;
/// Regex for parsing the output of "wpa_cli list_networks".
/// needs .multi_line(true).
const LIST_RE: &str = r#"^(\d+)\t[^\t]*\t([0-9a-f:]+)"#;
/// Regex for extracting passphrase from output of "wpa_passphrase".
const PASS_RE: &str = r#"\spsk=([0-9a-f]+)"#;

#[derive(Deserialize)]
struct ConfigFile {
    interface: Option<String>,
    library: Option<String>,
    wpa_socket: Option<String>,
    wpa_cli: Option<String>,
    wpa_conf: Option<String>,
    group: Option<String>
}

impl ConfigFile {
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

struct Config {
    interface: String,
    library: Utf8PathBuf,
    wpa_socket: Utf8PathBuf,
    wpa_cli: Utf8PathBuf,
    wpa_conf: Utf8PathBuf,
    group: String,
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
            wpa_conf,
            group: "netdev".to_owned(),
            dmx: Dmx::automagiconf(),
        }
    }
}

impl Config {
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
        if let Some(path) = cfgf.wpa_conf {
            cfg.wpa_conf = Utf8PathBuf::from(path);
        }
        if let Some(group) = cfgf.group {
            cfg.group = group;
        }
        
        cfg
    }
    
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
    
    fn wpa_cli_cmd(&self) -> Command {
        let mut cmd = Command::new(&self.wpa_cli);
        cmd.args(["-i", &self.interface, "-p", &self.wpa_socket.as_str()]);
        
        cmd
    }
    
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

#[derive(Debug, Serialize, Deserialize)]
struct WapCfg {
    mac: String,
    essid: String,
    pwd: String,
    psk: String,
}

impl WapCfg {
    fn to_wpa_conf_stanza(&self) -> String {
        format!(
            "network={{\n\tbssid={}\n\tssid=\"{}\"\n\t#psk=\"{}\"\n\tpsk={}\n}}\n",
            &self.mac, &self.essid, &self.pwd, &self.psk
        )
    }
}

#[derive(Debug)]
struct Wap {
    mac: String,
    freq: String,
    level: String,
    essid: String,
    old_essid: Option<String>,
    pwd: Option<String>,
    psk: Option<String>,
}

impl Wap {
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
        self.level.chars().count()
    }
    
    fn line(&self, key_len: usize) -> Vec<u8> {
        let config_char = match &self.psk {
            Some(_) => '*',
            None => ' ',
        };
        
        let mut line = format!(
            "{} {} {:>4} {:>width$} {}",
            config_char, &self.mac, &self.freq, &self.level, &self.essid,
            width = key_len
        );
        if let Some(id) = &self.old_essid {
            line.push(' ');
            line.push_str(id);
        }
        line.into_bytes()
    }
} 

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
    
    let stat = wpa_cli.wait()
        .map_err(|e| format!("Error awaiting wpa_cli subprocess: {}", &e))?;
    eprintln!("wpa_subprocess exited with {}", &stat);
    
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
    
    eprintln!("Matched {} Waps.", &waps.len());
    
    waps.sort_by_cached_key(|x| -x.level.parse::<i32>().unwrap());
    Ok(waps)
}

fn set_password(cfg: &Config, pwd: &str) -> Result<(), String> {
    let mut lib = load_library(&cfg.library).unwrap_or(Library::new());    
    let mut v = scan(&cfg, &lib)?;
    let n = match cfg.dmx.select("", &v)? {
        Some(n) => n,
        None => { return Ok(()); }
    };
    let wcfg = v.swap_remove(n).into_cfg(pwd)?;
    
    lib.insert(wcfg.mac.clone(), wcfg);
    save_library(&cfg.library, &lib)?;
    save_wpa_config(cfg, &lib)?;
    reconfigure(cfg)
}

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
            return match cfg.wpa_cli_cmd().args(["select_network", wap_n]).status() {
                Ok(_) => Ok(()),
                Err(e) => Err(format!(
                    "Error invoking wpa_cli to select_network {}: {}",
                    wap_n, &e
                )),
            };
        }
    }
        
    Err("Selected network not configured.".to_owned())
}

fn main() {
    let cfg = Config::new();
    
    if let Some(pwd) = std::env::args().nth(1) {
        if let Err(e) = set_password(&cfg, &pwd) {
            eprintln!("{}", &e);
        }
    } else {
        connect(&cfg).unwrap();
    }
}