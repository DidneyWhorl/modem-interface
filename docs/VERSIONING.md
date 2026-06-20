# Versioning & Release Convention

**Adopted:** 2026-04-21 (see `docs/superpowers/specs/2026-04-20-staging-environment-design.md`).

## Version shapes

| Shape | Meaning | Where it lives | Tagged? |
|---|---|---|---|
| `X.Y.0-dev.N` | Dev iteration on `main` toward next stable | `main` | No |
| `X.Y.0` | Stable minor release | `release/X.Y.x` HEAD | Yes, `vX.Y.0` |
| `X.Y.Z` (Z > 0) | Hotfix on an existing stable line | `release/X.Y.x` HEAD | Yes, `vX.Y.Z` |

## Normal dev cycle

```
main: 1.2.0-dev.1 → 1.2.0-dev.2 → ... → 1.2.0-dev.N
       │
       ├─ commit on main: bump to 1.2.0 (drop suffix)
       ├─ tag v1.2.0 on that commit
       ├─ cut release/1.2.x branch from the tag
       └─ commit on main: bump to 1.3.0-dev.1 (next dev series)
```

Tag and branch-cut live on the same commit. `main` bumps to the next dev series in the very next commit.

## Hotfix cycle

```
stable = release/1.2.x at 1.2.0; main at 1.3.0-dev.8

1. git checkout release/1.2.x
2. apply fix (cherry-pick from main if already there)
3. bump version to 1.2.1, commit
4. tag v1.2.1
5. git checkout main; cherry-pick the fix if not already there
```

`main` is never disturbed by hotfixes. Dev continues toward `v1.3.0` uninterrupted.

## Dev counter ceremony

`N` in `X.Y.0-dev.N` is bumped manually by the author as part of the normal version-bump
ritual (`frontend/package.json`, `backend/Cargo.toml`, `backend/Cargo.lock` via
`cargo update -p modem-interface`). CI does NOT auto-bump — version changes stay
intentional and reviewable.

## Packaging syntax translation

The canonical version lives in `package.json` / `Cargo.toml` as native semver. CI scripts
translate for each packaging format:

| Source | `1.2.0-dev.1` → |
|---|---|
| Rust/Cargo, npm | `1.2.0-dev.1` (native) |
| opkg | `1.2.0~dev1` (`~` sorts before anything in dpkg) |
| apk | `1.2.0_alpha1` (`_alpha` sorts before no suffix in apk) |

Ordering is preserved across all three: `X.Y.0-dev.N < X.Y.0 < X.Y.Z < (X.Y+1).0-dev.1`.

## CI trigger matrix (modem-interface)

| Event | Branch / Ref | CI action |
|---|---|---|
| Push | `main` | Build → publish `.apk`/`.ipk` at `X.Y.0-dev.N` to **testing feed** |
| Push | `release/X.Y.x` (no tag) | Build → clippy/tests only. No publish. |
| Tag push | `vX.Y.Z` on `release/*` | Build from tag → publish `.apk`/`.ipk` at `X.Y.Z` to **stable feed** |
| PR | any | Build → clippy/tests only. No publish. |

## Feed layout

```
https://packages.ctrl-modem.com/
├── stable/
│   ├── feed/<arch>/       # opkg
│   └── apk/<arch>/        # apk v3
└── testing/
    ├── feed/<arch>/
    └── apk/<arch>/
```

Legacy path `https://packages.ctrl-modem.com/{apk,feed}/<arch>/` is 301-redirected to `stable/...`
so existing routers continue to work with no config change.

## Retention

| Feed | Keep | Why |
|---|---|---|
| `testing/` | Latest build per arch only | Dev iterates fast; old dev builds have no value |
| `stable/` | Last 3 versions per arch | Enables pin-rollback via `apk add modem-interface=X.Y.Z` |

## Branch protection

- `main`: require CI pass before push.
- `release/*`: require CI pass, block force-push.
- Tags matching `v*`: immutable (protected-tags).
