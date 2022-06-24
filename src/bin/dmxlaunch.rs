/*!
`dmenu`-driven program launcher.

usage:

`dmxlaunch [ MENU_FILE ]`

If no `MENU_FILE` is provided on the command line, it will look for a menu
file in the following locations, in this order:

  * specified in the configuration file
  * `$XDG_CONFIG_HOME/dmxlaunch_menu.json`
  * `$HOME/.config/dmxlaunch_menu.json`

The configuration file allows the specification of the separator character
and a default menu file to use (if one isn't specified). The configuration
file will be sought (in this order):

  * at the value of `$DMXLAUNCH_CONFIG`
  * `$XDG_CONFIG_HOME/dmxlaunch.toml`
  * `$HOME/.config/dmxlaunch.toml`

The configuration file should have the format

```toml
separator = "/"
default_menu = "/home/dan/.config/dmxlaunch_menu.json"
```

If either of the options is omitted, it will be replace with the
default value (specified above).

*/
use std::path::Path;

use camino::{Utf8PathBuf};
use once_cell::sync::OnceCell;
use serde::{Deserialize};

use dm_x::{Dmx, Item};

static USAGE: &str = "
usage: dmxlaunch [ MENU_FILE ]
";

// The configuration struct has to be global because the separator information
// is used in the implementation of `dm_x::Item`.
static CFG: OnceCell<Config> = OnceCell::new();

// The only purpose for this struct is to be deserialized from a .toml file.
#[derive(Deserialize)]
struct ConfigFile {
    separator: Option<String>,
    default_menu: Option<String>,
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

// Contains global variables, including the `Dmx` configuration.
//
// Configuration comes from parsing a config file to get a `ConfigFile`,
// and also running `Dmx::automagiconf()`.
struct Config {
    separator: String,
    separator_length: usize,
    default_menu: Option<Utf8PathBuf>,
    dmx: Dmx,
}

impl Default for Config {
    fn default() -> Self {
        let default_menu = match dmxtools::config_directory() {
            Err(_) => None,
            Ok(mut pbuff) => {
                pbuff.push("dmxlaunch_menu.json");
                Some(pbuff)
            },
        };

        Self {
            separator: "/".to_owned(),
            separator_length: 1,
            default_menu,
            dmx: Dmx::automagiconf(),
        }
    }
}

impl Config {
    fn from_config_file(cfgf: ConfigFile) -> Config {
        let mut cfg = Config::default();
        if let Some(sep) = cfgf.separator {
            cfg.separator_length = sep.chars().count();
            cfg.separator = sep;
        }
        if let Some(menu) = cfgf.default_menu {
            cfg.default_menu = Some(Utf8PathBuf::from(menu));
        }
        cfg
    }
    
    fn new() -> Config {
        if let Ok(path) = std::env::var("DMXLAUNCH_CONFIG") {
            if let Some(cfgf) = ConfigFile::from_file(path) {
                return Config::from_config_file(cfgf);
            }
        }
        
        if let Ok(mut path) = dmxtools::config_directory() {
            path.push("dmxlaunch.toml");
            if let Some(cfgf) = ConfigFile::from_file(&path) {
                return Config::from_config_file(cfgf);
            }
        }
        
        Config::default()
    }
}

/*
Represents a runnable item. Meant to be deserialized from the menu file,
where it looks like this:

```json
{
    "key": "hx",
    "desc": "helix Text Editor",
    "exec": ["x-terminal-emulator", "-e", "/usr/local/bin/hx"]
}
```
*/
#[derive(Deserialize)]
struct Exec {
    pub key: String,
    pub desc: String,
    pub exec: Vec<String>,
}

/*
Represents a submenu. Meant to be deserialized from the menu file,
where it looks like this:

```json
{
    "key": "sys",
    "desc": "System Utilities",
    "entries": [
        # A list of Exec items and sub-Menu items goes here.    
    ]
}
```
*/
#[derive(Deserialize)]
struct Menu {
    pub key: String,
    pub desc: String,
    pub entries: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Entry {
    Exec(Exec),
    Menu(Menu),
}

impl Item for Entry {
    fn key_len(&self) -> usize {
        match self {
            Entry::Exec(x) => x.key.chars().count(),
            Entry::Menu(m) => m.key.chars().count(),
        }
    }
    
    fn line(&self, key_len: usize) -> Vec<u8> {
        let cfg = CFG.get().expect("Unconfigured!");
        
        match self {
            Entry::Exec(x) => format!(
                "{:key_width$}  {}\n",
                &x.key, &x.desc,
                key_width = key_len + cfg.separator_length
            ).into_bytes(),
            
            Entry::Menu(m) => format!(
                "{:key_width$}{}  {}\n",
                &m.key,
                &cfg.separator,
                &m.desc,
                key_width = key_len
            ).into_bytes()
        }
    }
}

// Attempt to deserialize a menu file, returning soemthing that can be passed
// to `Dmx::select()` (or, more pertinently, `recursive_select()`, below).
fn load_menu<P: AsRef<Path>>(path: P) -> Result<Vec<Entry>, String> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)
        .map_err(|e| format!("Error reading file \"{}\": {}", path.display(), &e))?;
    let entries: Vec<Entry> = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Error deserializing file \"{}\": {}", path.display(), &e))?;
    Ok(entries)
}

// Propt the user to choose an `Entry` with dmenu.
//
// If the user chooses an `Entry::Menu`, call this again on the list of
// `Entry`s in the selected submenu; if the user cancels, drop back up one
// menu level and reprompt at that level (or just return `None` if it's the
// top level).
fn recursive_select<'a>(prompt: &str, items: &'a [Entry]) -> Option<&'a Exec> {
    let cfg = CFG.get().expect("Unconfigured!");
    
    loop {
        match cfg.dmx.select(prompt, items).unwrap()
        {
            None => return None,
            Some(n) => match &items[n] {
                Entry::Exec(x) => { return Some(x.clone()); },
                Entry::Menu(m) => {
                    let new_prompt = format!("{}{}{}", prompt, &m.key, &cfg.separator);
                    if let Some(x) = recursive_select(&new_prompt, &m.entries) {
                        return Some(x);
                    }
                },
            },
        }
    }
}

// Given the Rust version of an `argv` of `chunks`, replace the current
// process with that program.
//
// This is trickier than just running a subprocess, which is kind of weird.
// You'd think it'd be simpler.
fn exec<S: AsRef<str>>(chunks: &[S]) -> ! {
    use std::ffi::CString;
    use std::os::raw::c_char;
    
    // Turn the command and arguments into a `Vec` of C-style strings
    // (null-terminated byte slices).
    let args: Vec<CString> = chunks.iter()
        .map(|c| CString::new(c.as_ref().as_bytes()).unwrap())
        .collect();
    // Create a `Vec` of pointers to our C-style strings.
    let mut arg_ptrs: Vec<*const c_char> = args.iter().map(|a| a.as_ptr()).collect();
    // Terminate our `Vec` of pointers with a null pointer. `execvp()` is going
    // to get passed a _pointer_ to our `Vec` of pointers (that's the way C
    // rolls, remember); this null pointer is required to signal the end
    // of the vector.
    arg_ptrs.push(std::ptr::null());
    // The pointer to the beginning of our `Vec` of pointers.
    let argv: *const *const c_char = arg_ptrs.as_ptr();
    
    // Here's a tricky part: The second argument to `execvp()` needs to be
    // the pointer to the array of pointers. The _first_ argument needs to
    // be _the first pointer in that array_. That particular value gets
    // passed _twice_: once as the first argument, and again as the first
    // element of the array pointed to by the second argument. Do you want
    // segfaults? 'Cause if you do this wrong, you'll get segfaults.
    let res = unsafe { libc::execvp(arg_ptrs[0], argv) };
    
    // `execvp()` shouldn't return, so we panic either way.
    if res < 0 {
        panic!("Error executing: {}", &res);
    } else {
        panic!("Exec... returned for some reason?");
    }
}

fn main() {
    CFG.set(Config::new()).map_err(|_| "Unable to set global CFG.").unwrap();
    
    let menu_file = match std::env::args().nth(1) {
        Some(path) => Utf8PathBuf::from(path),
        None => match &CFG.get().expect("Unconfigured!").default_menu {
            Some(path) => path.clone(),
            None => {
                eprintln!("No default menu file configured; must specify.{}", USAGE);
                std::process::exit(78);
            }
        },
    };
    
    let entries = match load_menu(&menu_file) {
        Ok(entz) => entz,
        Err(e) => {
            eprintln!("{}", &e);
            std::process::exit(65);
        }
    };
    
    if let Some(x) = recursive_select(
        &CFG.get().expect("Unconfigured!").separator,
        &entries
    ) {
        exec(&x.exec);
    }
}