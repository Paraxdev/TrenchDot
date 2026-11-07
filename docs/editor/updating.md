# Updating the editor

The editor you download from GitHub or itch.io updates itself from the
[rolling beta](https://github.com/Paraxdev/TrenchDot/releases/tag/beta). Once a day, when it starts, it asks GitHub
whether a newer build is out. It downloads nothing until you say so.

## Installing an update

When a newer build is out, a cyan **Update available** chip shows in the status bar at the bottom.

1. Click **Update available**, or choose *Help > Check for Updates…* to look right away. The *Editor Update* window
   says which build is out and which one you have.
2. Click **Download and Install**. The window shows the download progress, and **Cancel** stops it with the editor
   left as it was.
3. When it says the new build is installed, click **Restart Now**. If a map has unsaved changes, the editor asks
   about them first, and cancelling that question keeps the editor open on the old build.

The new build replaces the editor's executable in its folder. The old one stays beside it as
`trenchdot-previous.exe` on Windows or `trenchdot-previous` on Linux and macOS.

> [!TIP]
> A new editor build usually comes with a new Godot addon. After the restart, choose
> *Godot > Install or Update Addon…* to update the addon in your project too. The update window reminds you when the
> project's addon is older.

## Going back to the previous build

If the new build gives you trouble, choose *Help > Check for Updates…* and click **Roll Back**. It swaps the two
builds, so restart the editor to use the old one. Rolling back again returns to the newer build. Only one old build is
kept, the next update replaces it.

## Turning the daily check off

Go to *File > Preferences* and clear **Look for a newer build once a day** next to *Editor updates*. The same checkbox
sits at the bottom of the update window. *Help > Check for Updates…* still works with it off.

## When the editor does not update itself

The editor never looks for updates, and the update window tells you why, when:

| Case | What to do instead |
| --- | --- |
| You built it from source | Pull the newest code and build again, see [Development](../development.md) |
| A package manager installed it, for example Flatpak, Snap, AppImage, Homebrew or a folder under `/usr` | Update it the way that package manager does |
| Its folder is read only, for example under Program Files | Download the new build yourself from GitHub or [itch.io](https://paraxdev.itch.io/trenchdot), or move the editor to a folder you can write to |
| There is no ready made build for your platform | Download or build it yourself |

It also skips the check on start when it runs offscreen, with `--default-prefs`, with an MCP server from the command
line, or under CI, so test runs and agents never reach GitHub. A check that cannot reach GitHub, for example while
you are offline, is tried again on the next start rather than a day later.
