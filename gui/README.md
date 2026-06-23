# fsvfs - mount FromSoftware virtual filesystems

Access archived game files (Dark Souls, Armored Core, Elden Ring...) with this cross platform userspace filesystem framework without needing to unpack and store tens of gigabytes of (duplicate) data.

# About this GUI (fsvfs-gui)

Conveniently manages multiple mounts and mount configurations. Detects installed Steam games.

## Settings

- **Keys**: path to directory or nested directories with public RSA keys to use.
- **Dictionary**: path to directory or nested directories with hash dictionaries to use.
- **Cache**: path to directory where to store cached metadata to speed up DVDBND processing.
    **Use cache**: (if checked) read and store metadate to this cache.
    **Clear cache**: delete all cached metadata.
- **Select directory**: choose from a list of automatically detected Steam game directories or **Browse**.
    **Check** the DVDBNDs you would like to mount.
- **Mount point**: path to an *EXISTING* and *EMPTY* directory.
- **Game**: determines which keys and dictionary to use.
- **Mount filesystem**: mount **checked** DVDBNDs at the **mount point**, *which must be set*.

## Controls

- **Ctrl + O**: open (load) mount configuration.
- **Ctrl + S**: save current mount configuration.
