# User-scope distribution with installer-driven updates

## Context

Log Lens ships to Windows and Debian-based Linux desktops. Three requirements
shape distribution, and they pull against each other:

- **No admin rights.** Users are often on managed work machines where they
  cannot elevate. Nothing may write outside `$HOME` / `%LOCALAPPDATA%`.
- **Native feel.** The app must appear in alt-tab with its own icon and be
  searchable in the Windows Start menu and the Linux launcher — which means a
  real Start-menu shortcut and a real `.desktop` entry, not a loose binary.
- **Easy updates.** An in-app update manager, not "go download it again".

The obvious native format on each platform fails the first requirement:
`.deb` needs root, and a machine-wide MSI needs an administrator.

## Decision

Ship **per-user installs only**, and update by re-running the installer.

- **Windows:** an Inno Setup installer with `PrivilegesRequired=lowest`,
  installing into `%LOCALAPPDATA%\Programs\Log Lens` with a Start-menu
  shortcut and a per-user uninstall entry. No UAC prompt. A portable zip
  ships alongside it.
- **Linux:** a `.tar.gz` containing the binary plus an `install.sh` that
  copies into `~/.local/bin`, `~/.local/share/applications`, and
  `~/.local/share/icons/hicolor/…` — all XDG user paths, no root, and the
  launcher picks the app up after `update-desktop-database`.
- **Updates** download that platform's installer artifact, verify it against
  the Release's `SHA256SUMS`, and run it: `/SILENT` on Windows (with the
  Restart Manager closing and relaunching the app), `install.sh --quiet`
  followed by a re-exec on Linux. Both install directories are user-writable,
  so no elevation is needed at update time either.
- Each installer writes an `install-manifest.json` recording the install
  directory. A copy whose manifest is missing — or records a directory other
  than the one it is running from — is **Portable**, and is offered a link to
  the releases page instead of an in-app update.
- **No code signing.**

## Considered options

- **`.deb` package.** The native Debian format, `apt`-managed, familiar.
  Rejected: installation requires root, and a system-wide install into
  `/usr/bin` could not be updated by the app without elevation. Considered as
  a *secondary* artifact for users who want it and dropped — a second
  distribution channel that silently lacks self-update is a support burden
  out of proportion to its audience.
- **AppImage.** Single admin-free file, no install step. Rejected: it produces
  no launcher entry on its own. Desktop integration needs AppImageLauncher or
  a hand-written `.desktop`, so the "searchable in the launcher" requirement
  would go unmet for anyone who did not do extra work.
- **Per-user MSI (`ALLUSERS=""`).** Rejected: fussy to author, no better
  outcome than Inno, and worse control over shortcut and uninstall behaviour.
- **Portable zip only, on both platforms.** Rejected: nothing in the Start
  menu or launcher, which is most of what "native feel" meant here.
- **Hand-rolled binary swap for updates** (rename-then-move on Windows,
  unlink-then-move on Linux, no installer involved). Rejected: it updates the
  executable and nothing else, so shortcuts, the uninstall entry, the
  `.desktop` file, and the icon silently stop matching the installed version
  as soon as any of them changes. Reusing the installer keeps that metadata
  correct by construction and is less code.
- **Code signing** (OV certificate, ~$200–400/yr plus a hardware token or a
  cloud signing service). Rejected for now: it removes the SmartScreen wall,
  but OV still needs reputation to accumulate before warnings stop, and the
  current audience is small enough to tolerate one click-through. The release
  workflow is structured so a signing step can be slotted in later.

## Consequences

- **Windows users see a SmartScreen warning on first run** and must click
  "More info" → "Run anyway". This is documented in the README. It is the
  accepted cost of not signing.
- **No system-wide install exists.** Two users on one machine each install
  their own copy. Acceptable for a developer tool; it is also the only way to
  honour the no-admin constraint.
- **Portable copies cannot self-update.** They are notified and linked, never
  updated in place, because running the installer would create a second copy
  elsewhere while the user kept running the old one.
- **The artifact naming convention becomes a compatibility contract.** The
  update checker matches Release assets by name, so renaming them breaks
  self-update for every already-installed copy — precisely the population
  that cannot easily fix itself.
- **Update integrity rests on `SHA256SUMS` alone.** That catches corrupt or
  truncated downloads and a tampered CDN hop, but not an attacker who
  controls the GitHub Release itself, since they would control the checksums
  too. Signing is the fix if that threat ever matters.
