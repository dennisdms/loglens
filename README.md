# Log Lens

A cross-platform desktop IDE for browsing logs, built in Rust with
[iced](https://iced.rs). Its first iteration is built around querying
Elasticsearch: you configure a Connection, save named searches against it, and
read the results as a virtualized table or as templated raw text.

The vocabulary this document uses — Release, Artifact, Install flavour, Update
check — is defined in [`CONTEXT.md`](CONTEXT.md).

## Install

Every Release publishes the same three Artifacts plus a `SHA256SUMS` file
covering all of them. Grab them from the
[releases page](https://github.com/dennisdms/loglens/releases/latest):

| Artifact | Platform | Install flavour |
| --- | --- | --- |
| `LogLens-<version>-windows-x86_64-setup.exe` | Windows | Installer-managed |
| `LogLens-<version>-windows-x86_64-portable.zip` | Windows | Portable |
| `LogLens-<version>-linux-x86_64.tar.gz` | Linux (x86_64) | Installer-managed |

x86_64 only. There is no macOS build and no `aarch64` build.

### Windows

Two flavours ship, and the choice between them is about whether Log Lens gets
to own a directory on your machine:

- **`LogLens-<version>-windows-x86_64-setup.exe`** — the one to take unless you
  have a reason not to. It installs into
  `%LOCALAPPDATA%\Programs\Log Lens`, adds a Start-menu entry (and, if you tick
  the box, a desktop shortcut), registers an entry in Add/Remove Programs, and
  can update itself in place from a later Release.
- **`LogLens-<version>-windows-x86_64-portable.zip`** — for machines where you
  cannot or would rather not install anything: a USB stick, a locked-down box,
  a folder you plan to delete. Unzip it, run `loglens.exe`. Nothing is
  registered anywhere and there is nothing to uninstall. A portable copy is
  *told* when a newer Release exists but never replaces itself — running the
  installer would put a second copy in `%LOCALAPPDATA%` while you carried on
  running the one on the stick. To update it, download the new zip and replace
  `loglens.exe`.

Both flavours are the same binary, and the two can sit on one machine without
confusing each other.

To install: download the `.exe`, run it, and follow the wizard. No
administrator rights are required and no UAC prompt appears — see
[Per-user, no administrator](#per-user-no-administrator).

To check what you downloaded against the Release's `SHA256SUMS`:

```powershell
Get-FileHash .\LogLens-<version>-windows-x86_64-setup.exe -Algorithm SHA256
```

#### The SmartScreen warning

The first time you run either the installer or `loglens.exe`, Windows will show
a blue **"Windows protected your PC"** dialog with a **Don't run** button. This
is expected, and it is not a sign that the download failed or was tampered
with.

To proceed: click **More info**, then **Run anyway**.

The reason is simply that Log Lens is not code-signed. A signing certificate
costs a few hundred dollars a year and would still need to accumulate
reputation with SmartScreen before the warning stopped appearing, so for a
project this size the cost is not yet worth it — the reasoning is recorded in
[ADR 0003](docs/adr/0003-user-scope-distribution.md). SmartScreen is telling
you that Microsoft has not seen this file signed by a known publisher, which is
true of every unsigned build; it is not telling you the file is malicious. If
you would like to be sure of what you have, verify the download against the
Release's `SHA256SUMS` with the `Get-FileHash` command above.

Windows remembers the choice, so you will not be asked again on that machine.

### Linux (Debian-based)

**Minimum supported distributions: Ubuntu 22.04 or newer, Debian 12 or newer.**

This is a product decision, not a build detail. The release binary is compiled
against glibc 2.35, and a newer glibc runs older binaries but never the
reverse; anything older than those two releases will refuse to start with a
`GLIBC_... not found` error.

```sh
# Download LogLens-<version>-linux-x86_64.tar.gz and SHA256SUMS, then:
sha256sum --ignore-missing --check SHA256SUMS

tar xzf LogLens-<version>-linux-x86_64.tar.gz
cd LogLens-<version>-linux-x86_64
./install.sh
```

`install.sh` writes only under `$HOME` — it never needs `sudo` — and installs
four things:

| What | Where |
| --- | --- |
| binary | `~/.local/bin/loglens` |
| desktop entry | `~/.local/share/applications/io.github.dennisdms.LogLens.desktop` |
| icon | `~/.local/share/icons/hicolor/256x256/apps/io.github.dennisdms.LogLens.png` |
| install marker + uninstaller | `~/.local/share/loglens/` |

(`XDG_DATA_HOME` and `XDG_CONFIG_HOME` are honoured if you set them.)

Log Lens should appear in your launcher immediately. If `~/.local/bin` is not
on your `PATH` — which it is not on a fresh Debian — `install.sh` prints a note
saying so and shows the line to add to your shell profile. It deliberately does
not edit `.bashrc`, `.zshrc` or `.profile` for you. The launcher entry works
either way, because the installed `Exec=` is an absolute path.

`./install.sh --quiet` does the same thing without the commentary; that is the
mode the in-app Update uses.

## Per-user, no administrator

Both platforms install **per user**, into your own profile, and neither ever
asks to elevate:

- Windows: `%LOCALAPPDATA%\Programs\Log Lens`, with the Start-menu entry and
  the uninstall entry in your own registry hive
  (`PrivilegesRequired=lowest` — `/ALLUSERS` cannot talk the installer into a
  machine-wide install).
- Linux: `~/.local/bin` and `~/.local/share`, all XDG user paths.

Two consequences follow, and they are intentional:

- **There is no system-wide install.** Nothing lands in `/usr/bin`, `Program
  Files`, or `HKEY_LOCAL_MACHINE`.
- **Each user on a shared machine installs their own copy**, with their own
  Connections and their own settings.

The upside is that the install directory stays writable by you, which is what
lets Log Lens update itself without an administrator ever being involved. The
full reasoning, including why there is no `.deb` and no AppImage, is in
[ADR 0003](docs/adr/0003-user-scope-distribution.md).

## Uninstall

Uninstalling removes the program and **deliberately keeps your Connections,
Saved Searches, settings and stored secrets.** Reinstalling to fix a problem is
the most common reason anyone uninstalls, and destroying someone's configured
Connections and credentials for that would be hostile. Both uninstallers say so
on the way out.

If you genuinely want everything gone, remove the program first and then the
paths under [Removing settings and secrets](#removing-settings-and-secrets).

### Windows

- **Installer:** Settings → Apps → Installed apps → **Log Lens** → Uninstall.
  (Equivalently, run `unins000.exe` in `%LOCALAPPDATA%\Programs\Log Lens`.) The
  install directory, the Start-menu entry and any desktop shortcut go with it.
- **Portable:** delete the folder you unzipped.

### Linux

```sh
~/.local/share/loglens/uninstall.sh
```

`install.sh` copies the uninstaller there precisely so it is still findable
after the downloaded archive is gone. It removes the binary, the desktop entry,
the icon and the install marker, and nothing else. `--quiet` is accepted here
too.

### Removing settings and secrets

Two places, on both platforms: a config file, and the OS credential store.
Secrets are never written to the config file, so deleting it alone leaves your
stored passwords and API keys behind.

**Windows**

| What | Where |
| --- | --- |
| Connections, Saved Searches, settings | `%APPDATA%\loglens\config.json` |
| Crash log, if the app ever panicked | `%APPDATA%\loglens\loglens.log` |
| Connection secrets | Credential Manager → Windows Credentials → Generic Credentials, entries named `<connection-id>.loglens` |

Deleting the `%APPDATA%\loglens` folder covers the first two.

**Linux**

| What | Where |
| --- | --- |
| Connections, Saved Searches, settings | `~/.config/loglens/config.json` |
| Crash log, if the app ever panicked | `~/.local/share/loglens/loglens.log` |
| Connection secrets | the login keyring, as secret-service items under the service `loglens` (Seahorse shows them as `<connection-id>@loglens:default`) |

`rm -rf ~/.config/loglens ~/.local/share/loglens` covers the first two.

Where no keyring is available at all — a headless session, a locked-down
environment — Log Lens keeps secrets in memory for the run only and asks for
them again next time, so there is nothing on disk to remove.

## Cutting a Release

Releases are cut by pushing a tag. Everything else is
[`.github/workflows/release.yml`](.github/workflows/release.yml).

1. Bump `version` in `Cargo.toml` and commit it.
2. Tag and push:

   ```sh
   git tag v0.2.0
   git push origin v0.2.0
   ```

**The tag must be `v` + the `Cargo.toml` version, exactly.** The
`verify-version` job checks this before any toolchain is installed and fails
the whole workflow within seconds if they disagree — so `v0.2.0` requires
`version = "0.2.0"`, and `v0.2.0-rc.1` requires `version = "0.2.0-rc.1"`. A
Release whose tag and reported version disagree is a pipeline that lies to you,
and every bug report filed against it inherits the lie.

From there the workflow builds Linux on a pinned `ubuntu-22.04` runner (that
pin is what sets the minimum supported distributions above) and Windows on
`windows-latest`, runs each fresh binary with `--version` as a smoke test,
creates the Release as a **draft**, uploads all four assets, and only then
publishes it. If either platform fails to build, nothing is published at all —
fix it and re-tag. A partially-published Release would hand every user on that
platform a failed Update.

A tag containing `-rc` or `-beta` publishes as a **pre-release**. That is
load-bearing rather than cosmetic: GitHub's `/releases/latest` excludes
pre-releases server-side, which is the whole of how the Update check avoids
offering a release candidate to users. There is no client-side filter to get
wrong.

Artifact names are a compatibility contract, not a formatting choice. The
Update check matches Release assets by name, so renaming one breaks self-update
for every already-installed copy — the single population that cannot fix
itself.

Re-running the workflow against a tag that already has a Release fails at the
create step, on purpose. Delete that Release by hand, or re-tag.

## Building from source

Requires a stable Rust toolchain. Nothing in the tree links a C library.

```sh
cargo run            # debug build, with a console on Windows
cargo build --release
```

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and
`cargo test` are what CI runs on every push.
