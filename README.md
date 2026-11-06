# gop-rotate

A UEFI boot services driver that rotates the firmware text console for devices with
sideways portrait panels, like the GPD Pocket 3 and 4.

`gop-rotate` finds the portrait GOP framebuffer, installs a custom `SimpleTextOutput`
protocol that renders rotated glyphs via `Blt`, and replaces `ConOut` and `StdErr`
in the EFI system table. The underlying GOP mode is left untouched so the kernel's
EFI stub still sees the real panel dimensions.

## Modes

| Mode | Grid                                  | Glyph cell                  |
| ---- | ------------------------------------- | --------------------------- |
| 0    | 80x25                                 | 28x64 on a 2560x1600 screen |
| 1    | 80x50                                 | 14x32                       |
| 2    | fills the screen, 182x50 on 2560x1600 | 14x32                       |

Mode 2 is the default. You can press `r` in systemd-boot to cycle modes, or set
`console-mode` in `loader.conf`. Glyphs are rendered with `noto-sans-mono-bitmap`,
which doesn't include box-drawing characters, so systemd-boot falls back to ASCII
borders.

## Install

```sh
nix build github:amaanq/gop-rotate
```

Copy `result/gop-rotate-x64.efi` to `\EFI\systemd\drivers\` on your ESP. systemd-boot
automatically loads any driver there ending in `x64.efi`. If Secure Boot is enabled,
sign the binary with your key in `db` first.

By default, the build rotates 90 degrees clockwise. You can override this by setting
`GOP_ROTATE_DEGREES` to `90`, `180`, or `270`, or by passing `rotation` to `nix/package.nix`.
For 90 and 270 degree rotations, the driver only attaches to portrait GOP modes
and unloads itself on landscape displays.
