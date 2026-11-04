# BLACKVAULT — Voice Commands & Automation

Every command here is editable. Open **Voice → Voice Command Editor** to change a
phrase, add a new one, disable one, or bind one to a different action. The list
below is what BLACKVAULT ships with on first run; nothing is fixed.

## 1. Wake phrase

Default: **"black vault"**. Say it, then say a command within six seconds.

Toggle it in **Settings → Voice engine**. With *Require wake phrase for every
command* off, commands run without the wake word — convenient for a quiet room,
noisy in a crowd. The lock screen ignores this setting entirely: there, only
unlock-class phrases are considered.

## 2. Core commands

| Phrase | Action | Notes |
|---|---|---|
| "lock this PC" | Lock | Deploys Full-Screen Privacy Mode on every monitor |
| "unlock this PC" | Unlock | Only meaningful on the lock screen; triggers the challenge |

## 3. Media and volume

| Phrase | Action | Key sent |
|---|---|---|
| "next" | Next track / next video | `VK_MEDIA_NEXT_TRACK` |
| "previous" | Previous track | `VK_MEDIA_PREV_TRACK` |
| "play" | Play / pause | `VK_MEDIA_PLAY_PAUSE` |
| "pause" | Play / pause | `VK_MEDIA_PLAY_PAUSE` |
| "volume up" | Volume up | `VK_VOLUME_UP` |
| "volume down" | Volume down | `VK_VOLUME_DOWN` |
| "mute" | Mute | `VK_VOLUME_MUTE` |

These are the real system media keys, so they work with whatever has media
focus — a browser video, a music player, a podcast app.

## 4. Pointer and keyboard

| Phrase | Action | Payload |
|---|---|---|
| "click" | Left-click at the current cursor position | — |
| "scroll down" | Scroll down | `down:600` |
| "scroll up" | Scroll up | `up:600` |
| "press enter" | Press Enter | `enter` |

`click` sends a real `SendInput` click wherever the pointer already is. To click
a fixed point, edit the command's payload to `x,y` (for example `640,360`) and
it will move the cursor there first.

## 5. Desktop

| Phrase | Action | Key sent |
|---|---|---|
| "show desktop" | Show the desktop | `Win+D` |
| "minimise all" | Minimise every window | `Win+M` |
| "take a screenshot" | Capture the primary monitor | writes a BMP to `Pictures\BLACKVAULT` |
| "open settings" | Open Windows Settings | `ms-settings:` |

## 6. Application launcher

| Phrase | Action |
|---|---|
| "open chrome" | Launch the launcher entry named *Chrome* |
| "open notepad" | Launch *Notepad* |
| "open file explorer" | Launch *File Explorer* |

These ship as examples. Add your own in **Launcher → Add Application**, or scan
the Start Menu with **Scan Installed Apps** and add from the list. A launch
command resolves by exact name, then by voice phrase, then by substring — and it
will only ever launch something on the approved list. Anything else returns
*"… is not in the approved launcher list"* and is logged.

## 7. Action types available in the editor

When you create a command you pick one of these action types:

| Action type | What the payload means | Example payload |
|---|---|---|
| `lock` | — | — |
| `unlock` | — | — |
| `launch_app` | app name or voice phrase | `Chrome` |
| `close_app` | process name, `.exe` optional | `chrome` |
| `open_url` | a URL the shell may open | `https://example.com` |
| `key_press` | a key combo | `ctrl+shift+esc` |
| `mouse_click` | empty, or `x,y` | `640,360` |
| `scroll` | `direction:amount` | `down:600` |
| `media_next` | — | — |
| `media_prev` | — | — |
| `media_play_pause` | — | — |
| `volume_up` | — | — |
| `volume_down` | — | — |
| `volume_mute` | — | — |
| `run_program` | full path to an `.exe` | `C:\Tools\thing.exe` |
| `open_folder` | a folder path | `D:\Projects` |
| `minimize_all` | — | — |
| `show_desktop` | — | — |
| `screenshot` | — | — |

### Key-combo syntax

Modifiers are `ctrl`, `shift`, `alt`, `win`, joined with `+`, followed by one
key: a letter, a digit, `enter`, `esc`, `space`, `tab`, `backspace`, `delete`,
`insert`, `home`, `end`, `pageup`, `pagedown`, `left`, `up`, `right`, `down`,
`f1`–`f12`, or the punctuation names `semicolon`, `equals`, `comma`, `minus`,
`period`, `slash`, `backtick`, `lbracket`, `backslash`, `rbracket`, `quote`.

Examples: `win+d`, `ctrl+shift+esc`, `alt+f4`, `ctrl+alt+delete` is **not**
permitted — see below.

## 8. What BLACKVAULT will not do

- It will not send `Ctrl+Alt+Del`. That sequence belongs to Windows; synthesising
  it is blocked by the OS and attempting to work around that would be a security
  hole.
- It will not type into a window running at a higher integrity level than
  itself. If you launch BLACKVAULT as a normal user and an application runs
  elevated, `SendInput` will not reach it. That is Windows UIPI working
  correctly.
- It will not open a URL scheme other than `http`, `https`, `ms-settings`,
  `mailto` and `ms-windows-store`.
- It will not launch a file that is not a real `.exe`, and it never launches
  through a shell, so an argument cannot be reinterpreted as a command.

## 9. Commands that need authentication

Tick *Require a local factor before running* on a command and it will only run
within five minutes of a successful authentication. Use this for anything that
touches money, mail or files — a voice is easy to imitate from a recording, a
PIN is not.
