/*!
Some common functionality for several of the `dmxtools` tools.
*/
use camino::Utf8PathBuf;

pub fn config_directory() -> Result<Utf8PathBuf, &'static str> {
    use std::env::var;
    
    match var("XDG_CONFIG_HOME") {
        Ok(p) => Ok(Utf8PathBuf::from(p)),
        Err(_) => match var("HOME") {
            Ok(home) => {
                let mut pbuff = Utf8PathBuf::from(home);
                pbuff.push(".config");
                Ok(pbuff)
            },
            Err(_) => Err("Unable to determine configuration directory.")
        }
    }
}
