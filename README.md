# fsvfs - mount FromSoftware virtual filesystems

Access archived game files (Dark Souls, Armored Core, Elden Ring...) with this cross platform userspace filesystem framework without needing to unpack and store tens of gigabytes of (duplicate) data.

This is a work in progress, the CLI is unstable and file integrity is on a best effort basis (until I introduce integrity checking at least) so YMMV.

## Requirements

A x86-64-v3 capable CPU (AVX2 and BMI2 ISAs). This requirement may be relaxed in the future as it is purely for performance.

### Linux

Kernel with [FUSE](https://www.kernel.org/doc/html/next/filesystems/fuse.html) support. fsvfs fully relies on [fuser](https://crates.io/crates/fuser/0.17.0) to handle the kernel communication, see its README for more info.

### Windows

[ProjFS](https://learn.microsoft.com/en-us/windows/win32/projfs/projected-file-system), an optional Windows feature. fsvfs will attempt to enable it, which may ask you for Administrator priveleges in PowerShell. A restart of fsvfs or your system may be required right after and the feature stays enabled going forward.

## Usage

Show help:
```bash
fsvfs --help
```

Mount all Elden Ring archives in an empty subdirectory named "eldenring" (bash):
```bash
fsvfs dvdbnd --mountpoint eldenring \
    "$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game/Data0.bhd" \
    "$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game/Data1.bhd" \
    "$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game/Data2.bhd" \
    "$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game/Data3.bhd" \
    "$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game/DLC.bhd"   \
    "$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game/sd/sd.bhd" \
    "$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game/sd/sd_dlc02.bhd"
```

Mount all Elden Ring archives in an empty subdirectory named "eldenring" (PowerShell):
```pwsh
.\fsvfs.exe dvdbnd --mountpoint eldenring `
    "G:\SteamLibrary\steamapps\common\ELDEN RING\Game\Data0.bhd" `
    "G:\SteamLibrary\steamapps\common\ELDEN RING\Game\Data1.bhd" `
    "G:\SteamLibrary\steamapps\common\ELDEN RING\Game\Data2.bhd" `
    "G:\SteamLibrary\steamapps\common\ELDEN RING\Game\Data3.bhd" `
    "G:\SteamLibrary\steamapps\common\ELDEN RING\Game\DLC.bhd"   `
    "G:\SteamLibrary\steamapps\common\ELDEN RING\Game\sd\sd.bhd" `
    "G:\SteamLibrary\steamapps\common\ELDEN RING\Game\sd\sd_dlc02.bhd"
```

NOTE: the mountpoint MUST be an empty directory.

## License
Licensed under either of

 * Apache License, Version 2.0
   ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
 * MIT license
   ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
