# dmxtools

Some Rust utilities that use [`dmenu`](https://tools.suckless.org/dmenu/),
interfacing with it through the [`dm_x` crate](https://github.com/d2718/dmx-rs).

This crate contains:

## `dmxcm`

A command-line clipboard manager:

```text
usage: dmxcm [ OPERATION ]
where OPERATION is one of the following:
  -s, --save      save the contents of the X clipboard
  -r, --recall    recall a saved clip into the X clipboard
  -d, --delete    delete a saved clip
  -x, --expunge   delete all saved clipboard values
```

I bind `$mod-c` and `$mod-v` to `dmxcm -s` and `dmxcm -r` in
[`i3`](https://i3wm.org/) as a textual copy-paste on steroids.

## `dmxlaunch`

A program-launcher. Parses a nested JSON file to present a series of
hierarchical menus. I have `$mod-z` bound to
`dmxlaunch /home/dan/.config/dmxlaunch_menu.json` in `i3`.

Her is an abriged launcher menu file example:

```json
[
{
    "key":"apps",
    "desc": "Heavyweight System Applications",
    "entries": [
        {
            "key": "ff",
            "desc": "Firefox ESR (Debian Package)",
            "exec": ["/usr/bin/firefox"]
        },
        {
            "key": "gftp",
            "desc": "gFTP FTP Client",
            "exec": ["/usr/bin/gftp-gtk"]
        },
        {
            "key": "soffice",
            "desc": "LibreOffice Suite",
            "exec": ["/usr/bin/soffice"]
        }
    ]
},
{
    "key": "sys",
    "desc": "System Utilities",
    "entries": [
        { 
            "key": "arandr",
            "desc": "Visual Frontend to XRandR",
            "exec": ["/usr/bin/arandr"]
        },
        {
            "key": "sshot",
            "desc": "Take a Screenshot (5 sec delay)",
            "exec": ["/usr/bin/lua", "home/dan/.config/i3/aux_scripts/sshot.lua", "5"]
        }
    ]
},
{
    "key": "wx",
    "desc": "Current Local Weather",
    "exec": ["/usr/bin/luajit", "/home/dan/dev/wx/wx.lua", "-b"]
},
{
    "key": "wifi",
    "desc": "Wireless Network Selector",
    "exec": ["/home/dan/.local/bin/dmxwifi"]
}
]
```

Entries with an `"entries"` key will themselves be submenus, and selecting them
will present a selection of the items contained in the `"entries"` list.
Entries with an `"exec"` key will have the values in that list executed; the
first item being the path to the program, and subsequent items being the
command line arguments.

## `dmxwifi`

A frontend and librarian for
[`wpa_supplicant`](https://wiki.archlinux.org/title/wpa_supplicant). (I don't
use Arch, btw, but its wiki is the best.)

Running

```bash
dmxwifi
```

by itself will display a menu of detectable wifi ESSIDs;
entries beginning with a `*` have saved passwords and can be selected and
joined.

```bash
dmxwifi -p "wifi_password_here"
```

will present a menu of detectable wifi ESSIDs and allow you to select which
one you want to associate the provided password with. You can then run
`dmxwifi` again and select it from the list (it should have an asterisk now)
to join it.