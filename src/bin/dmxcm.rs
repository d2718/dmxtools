/*!
A (text-only) clipboard manager using dmenu

See `const USAGE` below for invocation.

Attempts to read configuration from two files:

`$XDG_CONFIG_HOME/.config/dmx.toml` for `dm_x` configuration.
(See the `dm_x` crate for format and details.)

`$XDG_CONFIG_HOME/.config/dmxcm.toml` which could contain up to the
following three options:

`
# Maximum width of lines shown in dmenu
max_width = 120
# Directory to store clipboard clips (replace 1000 with your UID)
clips_dir = "/run/user/1000/dmxcm"
# Path to the xclip program (the default of "xclip" is fine if it's
# in your path).
xclip = "xclip"
`

Any omitted options will be replaced with the defaults above.
*/
use std::io::Write;
use std::process::{Command, Stdio};

use camino::{Utf8Path, Utf8PathBuf};
use once_cell::sync::OnceCell;
use serde::{Deserialize};
use dm_x::{Dmx, Item};

const ELLIPSIS: char = '\u{2026}';

const USAGE: &str = "
usage: dmxcm [ OPERATION ]

where OPERATION is one of the following:

  -s, --save      save the contents of the X clipboard
  -r, --recall    recall a saved clip into the X clipboard
  -d, --delete    delete a saved clip
  -x, --expunge   delete all saved clipboard values
";

static CFG: OnceCell<Config> = OnceCell::new();

fn die(message: &str) -> ! {
    std::io::stderr().write(message.as_bytes()).unwrap();
    std::process::exit(1);
}

// fn get_config_directory() -> Result<Utf8PathBuf, String> {
//     use std::env::var;
    
//     match var("XDG_CONFIG_HOME") {
//         Ok(p) => Ok(Utf8PathBuf::from(p)),
//         Err(_) => match var("HOME") {
//             Ok(home) => {
//                 let mut pbuff = Utf8PathBuf::from(home);
//                 pbuff.push(".config");
//                 Ok(pbuff)
//             },
//             Err(_) => Err("Unable to determine configuration directory".to_owned())
//         },
//     }
// }

#[derive(Deserialize)]
struct ConfigFile {
    pub max_width: Option<usize>,
    pub clips_dir: Option<String>,
    pub xclip: Option<String>,
}

#[derive(Debug)]
struct Config {
    max_width: usize,
    clips_dir: Utf8PathBuf,
    xclip: Utf8PathBuf,
}

impl Default for Config {
    fn default() -> Config {
        let uid_out = Command::new("id").arg("-u").output()
            .map_err(|e| {
                let estr = format!("Unable to determine UID: {}", &e);
                die(&estr);
            }).unwrap()
            .stdout;
        let trimmed_uid = std::str::from_utf8(&uid_out)
            .map_err(|_| die("Output of `uid -u` not UTF-8.")).unwrap()
            .trim();
        let clips_dir: Utf8PathBuf = ["/", "run", "user", trimmed_uid, "dmxcm"]
            .iter().collect();
        
        Config {
            max_width: 120,
            clips_dir,
            xclip: "xclip".into(),
        }
    }
}

fn configure_dmxcm() -> Result<Config, String> {
    let mut config_path = dmxtools::config_directory()?;
    config_path.push("dmxcm.toml");
    
    let bytes = std::fs::read(&config_path)
        .map_err(|e| format!(
            "Unable to read dmxcm configuration file {}: {}.",
            &config_path, &e
        ))?;
    
    let usr_cfg: ConfigFile = toml::from_slice(&bytes)
        .map_err(|e| format!(
            "Error deserializing dmxcm configuration file {}: {}.",
            &config_path, &e
        ))?;
    
    let mut cfg = Config::default();
    if let Some(width) = usr_cfg.max_width {
        cfg.max_width = width;
    }
    if let Some(dir) = usr_cfg.clips_dir {
        cfg.clips_dir = Utf8PathBuf::from(dir);
    }
    if let Some(path) = usr_cfg.xclip {
        cfg.xclip = Utf8PathBuf::from(path);
    }
    
    Ok(cfg)
}

/*
Return a copy of `text` with surrounding whitespace stripped and all
sequences of interior whitespace collapsed down to a single space.

Limit the length to `max_len` characters, and set the final character
to an ellipsis if it would exceed that length.
*/
fn collapse_whitespace(text: &str, max_len: usize) -> String {
    let mut out_chars: Vec<char> = Vec::with_capacity(max_len);
    let mut last_char_was_ws: bool = true;
    let mut chars = text.trim().chars();

    while let (Some(c), true) = (chars.next(), out_chars.len() < max_len) {
        if c.is_whitespace() {
            if !last_char_was_ws {
                out_chars.push(' ');
                last_char_was_ws = true;
            }
        } else {
            out_chars.push(c);
            last_char_was_ws = false;
        }
    }
    
    if let Some(_) = chars.next() {
        let _ = out_chars.pop();
        out_chars.push(ELLIPSIS);
    }
    
    let output: String = out_chars.into_iter().collect();
    output
}

/*
An `Entry` represents a single saved clipboard item, and holds a path
to the file as well as the file's contents.
*/
struct Entry {
    path: Utf8PathBuf,
    // This makes them easily sortable.
    n: usize,
    contents: String,
}

impl Entry {
    // Instantiate an `Entry` from the path of a file in the clip directory.
    fn from_path(path: &Utf8Path) -> Result<Entry, String> {
        let n: usize = path.file_name()
            .ok_or(format!("Path \"{}\" has no filename.", &path))?
            .parse()
            .map_err(|e| format!("Path \"{}\" filename can't be parsed as usize: {}", &path, &e))?;
        
        let contents = std::fs::read_to_string(&path)
            .map_err(|e| format!("Unable to read \"{}\": {}", &path, &e))?;
        
        let ent = Entry {
            path: path.to_path_buf(),
            n,
            contents,
        };
        
        Ok(ent)
    }
}

impl Item for Entry {
    fn key_len(&self) -> usize {
        self.path.as_path().file_name()
            .unwrap_or_else(|| die("Directory Entry should have a file_name()."))
            .chars().count()
    }
    
    fn line(&self, key_len: usize) -> Vec<u8> {
        let max_len = CFG.get().unwrap().max_width;
        let collapsed = collapse_whitespace(&self.contents, max_len);
        let linestr = format!(
            "{:0>width$}  {}",
            &self.path.file_name().unwrap(),
            &collapsed,
            width = key_len
        );
        linestr.into_bytes()
    }
}

/*
Return a Vec of `Entry`s representing all the saved clips in the clip
directory.
*/
fn read_entries(dir: &Utf8Path) -> Result<Vec<Entry>, String> {
    let mut entries: Vec<Entry> = Vec::new();
    
    for path in dir.read_dir_utf8()
        .map_err(|e| format!("Unable to read directory \"{}\": {}", &dir, &e))?
    {
        if let Ok(p) = path {
            match Entry::from_path(p.path()) {
                Ok(e) => { entries.push(e); },
                Err(e) => { eprintln!("{}", &e); },
            }
        }
    }
    
    Ok(entries)
}

/*
Write the contents of the X clipboard to a file in the clip directory with
the given number.
*/
fn save_clipboard_to_file_n(dir: &Utf8Path, n: usize) -> Result<(), String> {
    let xclip = &CFG.get().unwrap().xclip;
    let output = Command::new(xclip).arg("-o").output()
        .map_err(|e| format!("Error running xclip process: {}", &e))?
        .stdout;
    let mut path = dir.to_path_buf();
    path.push(n.to_string());
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(&path)
        .map_err(|e| {
            format!(
                "Unable to open \"{}\" for create/truncate/write: {}",
                &path, &e
            )
        })?;

    f.write_all(&output)
        .map_err(|e| format!("Error writing to \"{}\": {}", &path, &e))
}

/*
Insert the contents of the given `Entry` into the X clipboard.
*/
fn pipe_entry_to_clipboard(ent: &Entry) -> Result<(), String> {
    let xclip = &CFG.get().unwrap().xclip;
    let mut child = Command::new(xclip)
        .args(&["-i", "-selection", "clipboard"])
        .stdin(Stdio::piped()).spawn()
        .map_err(|e| format!("Unable to spawn xclip process: {}", &e))?;
    {
        let mut handle = child.stdin.take()
            .ok_or("xclip child process stdin handle unavailable.")?;
        handle.write_all(&ent.contents.as_bytes())
            .map_err(|e| format!("Error writing to xclip process: {}", &e))?;
    }
    let status = child.wait()
        .map_err(|e| format!("Error awaiting xclip process: {}", &e))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("xclip process returned exit code {:?}", &status.code()))
    }
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| die(USAGE));

    let cfg = match configure_dmxcm() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("{} Using defaults.", &e);
            Config::default()
        }
    };
    CFG.set(cfg).unwrap();
    
    std::fs::create_dir_all(&CFG.get().unwrap().clips_dir)
        .expect("Unable to guarantee existence of clipboard directory.");
    
    let mut entries = read_entries(&CFG.get().unwrap().clips_dir)
        .expect("Unable to read entries from the clipboard directory.");
    
    match arg.as_str() {
        
        "-r" | "--recall" => {
            let dmx = Dmx::automagiconf();            
            entries.sort_unstable_by(|a, b| b.n.cmp(&a.n));
            
            if let Some(n) = dmx.select("▶", &entries).unwrap() {
                pipe_entry_to_clipboard(&entries[n]).unwrap();
            }
        },
        
        "-s" | "--save" => {
            let new_n = match entries.iter().map(|ent| ent.n).max() {
                Some(n) => n + 1,
                None => 0,
            };
            save_clipboard_to_file_n(&CFG.get().unwrap().clips_dir, new_n).unwrap();
        },
        
        "-d" | "--delete" => {
            let dmx = Dmx::automagiconf();
            entries.sort_unstable_by(|a, b| b.n.cmp(&a.n));
            
            if let Some(n) = dmx.select("⏏", &entries).unwrap() {
                let ent = &entries[n];
                if let Err(e) = std::fs::remove_file(&ent.path) {
                    eprintln!("Error removing clipboard file {}: {}", &ent.path, &e);
                }
            }
        },
        
        "-x" | "--expunge" => {
            for ent in entries.iter() {
                if let Err(e) = std::fs::remove_file(&ent.path) {
                    eprintln!("Error removing clipboard file {}: {}", &ent.path, &e)
                }
            }
        }
        
        _ => {
            print!("{}", USAGE);
        },
    }
}