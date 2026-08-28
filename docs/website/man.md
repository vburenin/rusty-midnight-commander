# Manual pages

These are original Apache-2.0 short manuals for the *shipped* CLI. They
are not copies of GNU `mc(1)`, `mcedit(1)`, `mcview(1)`, or `mcdiff(1)`.

HTML twin: [man.html](man.html).

| Page | Covers | Start as |
| --- | --- | --- |
| [`mcr(1)`](../man/mcr.1) | Dual-pane file manager, flags, archive browse + copy-out | `mcr` |
| [`mcr-edit(1)`](../man/mcr-edit.1) | Internal editor keys (mcedit equivalent) | `mcr --edit` / F4 |
| [`mcr-view(1)`](../man/mcr-view.1) | Internal viewer keys (mcview equivalent) | `mcr --view FILE` / F3 |
| [`mcr-diff(1)`](../man/mcr-diff.1) | Side-by-side diff keys (mcdiff equivalent) | `mcr --diff FILE1 FILE2` |

## Read them in a terminal

```bash
man -l docs/man/mcr.1
man -l docs/man/mcr-edit.1
man -l docs/man/mcr-view.1
man -l docs/man/mcr-diff.1
```

After installing into `man1` (see [install.md](install.md)): `man mcr`,
`man mcr-edit`, `man mcr-view`, `man mcr-diff`. `mcr --help` prints the
same access hints. F1 inside the TUI is a separate hypertext help tree
under `data/help/`.

## Groff sources in the tree

- [`docs/man/mcr.1`](../man/mcr.1)
- [`docs/man/mcr-edit.1`](../man/mcr-edit.1)
- [`docs/man/mcr-view.1`](../man/mcr-view.1)
- [`docs/man/mcr-diff.1`](../man/mcr-diff.1)

On GitHub those links open the groff source as text. They are the
manuals this project ships; this page only indexes them.
