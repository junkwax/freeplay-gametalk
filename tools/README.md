# Tools

Helper scripts for local release preparation.

## FBNeo Core

Build the platform libretro core into `cores/`:

```powershell
.\tools\build-fbneo-windows.ps1
```

```bash
./tools/build-fbneo-linux.sh
./tools/build-fbneo-macos.sh
```

Requirements:

- Git
- `make` or `mingw32-make`
- A working C/C++ toolchain for the target platform
- On Windows, MSYS2/MinGW is the expected path

The scripts clone/update `vendor/FBNeo`; `vendor/` and `cores/` are ignored.

## Packaging

Windows:

```powershell
.\package.ps1
```

Linux/macOS:

```bash
./package-linux.sh
./package-macos.sh
```

Package scripts do not include ROM files or private `.env` values.
