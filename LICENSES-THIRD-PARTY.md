# Third-Party Dependency Licenses

This project bundles a number of third-party dependencies. The large majority
are distributed under permissive licenses (MIT, Apache-2.0, BSD, ISC). This
document records (a) the elected branch for any dependency that is offered under
multiple licenses and (b) weak-copyleft dependencies that are used unmodified.

## Multi-licensed dependencies — elected branch

| Dependency | Offered under | Elected |
| --- | --- | --- |
| unescaper | GPL-3.0 OR MIT | MIT |
| r-efi | MIT OR Apache-2.0 OR LGPL | MIT / Apache-2.0 |

## Weak-copyleft dependencies (used unmodified)

The `serialport` and `option-ext` crates are licensed under MPL-2.0 and are used
as unmodified upstream dependencies. MPL-2.0 is file-level copyleft: it applies
only to those upstream files. No project source is placed under MPL-2.0.

## Notes

This document is a summary of the project's license elections, not the complete
dependency manifest. Full per-dependency license text ships with each dependency
in its source distribution.
