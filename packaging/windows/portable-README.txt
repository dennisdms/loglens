Log Lens — portable
===================

This is the portable flavour of Log Lens: a single executable, unpacked by
hand, owned by nobody. There is nothing to install and nothing to uninstall.
Run loglens.exe from wherever you put it — a USB stick, a network share, a
folder in your profile. It writes nothing outside the two places listed below.


First run: SmartScreen
----------------------

Log Lens is not code-signed, so Windows SmartScreen will show a
"Windows protected your PC" dialog the first time you run it. Click
"More info", then "Run anyway". You will not be asked again on that machine.


Updates
-------

A portable copy cannot update itself. Log Lens still tells you when a newer
release exists and links you to the releases page, but it will not replace
itself in place: running the installer would put a *second* copy into
%LOCALAPPDATA%\Programs\Log Lens while you carried on running this one.

To update, download the new portable archive and replace loglens.exe.

    https://github.com/dennisdms/loglens/releases

If you would rather have a copy that updates itself, a Start menu entry and an
Add/Remove Programs entry, use the installer instead — it needs no administrator
rights either:

    LogLens-<version>-windows-x86_64-setup.exe

The two can coexist. This directory deliberately contains no
install-manifest.json, which is how Log Lens tells the two apart: an installed
copy carries one recording the directory it lives in, and only a copy running
from that exact directory is allowed to update itself.


Where your data lives
---------------------

Neither flavour keeps anything beside the executable:

  Connections, Saved Searches, settings
      %APPDATA%\loglens\config.json

  Connection secrets (passwords, API keys)
      Windows Credential Manager

  Crash log, if the app ever panics
      %APPDATA%\loglens\loglens.log

Deleting this folder therefore leaves your settings and secrets behind. Remove
them by hand if you want them gone.


Checking what you downloaded
----------------------------

Every release ships a SHA256SUMS file covering all of its archives:

    Get-FileHash .\LogLens-<version>-windows-x86_64-portable.zip -Algorithm SHA256
