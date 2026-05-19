# cargo-remove-unused-derives

A Cargo subcommand that finds and removes unused `#[derive(...)]` entries from
your project, guided by the compiler.

Unused derives accumulate over time: traits get added "just in case", code that
needed them moves away, and the compiler stays silent because the derive itself
still compiles. This tool sandbox-copies your project, strips every derive,
then uses `cargo check` diagnostics to put back only the ones that are
actually needed.

## Install

```sh
cargo install cargo-remove-unused-derives
```

Requires Rust 1.91+ (edition 2024).

## Usage

```sh
# Dry run — print what would be removed, touch nothing.
cargo remove-unused-derives

# Apply the changes in place.
cargo remove-unused-derives --write
```

Common flags:

| Flag | Effect |
| --- | --- |
| `--write` | Rewrite source files. Without this, the tool only reports. |
| `-p, --package <NAME>` | Limit analysis to specific package(s). Can be repeated. |
| `--allow-dirty` | Allow `--write` when the git working tree has uncommitted changes. |
| `--allow-no-vcs` | Allow `--write` when not inside a git repository. |
| `--strict` | Fail if any unused derive can't be confidently pinpointed (see caveat below). |
| `-v, --verbose` | Print progress and debug output to stderr. |

## Sample output

```
$ cargo remove-unused-derives
src/handler.rs:14 RequestParams — unused: PartialEq, Eq
src/state.rs:42 Session — unused: Hash
```

With `--write`, the same diagnostics are printed and the files are updated.
Re-running on a clean tree should report `No unused derives found.`

## A word on `--write`

`--write` modifies source files in place. By default the tool refuses to do
that unless you're inside a git repository with a clean working tree — pass
`--allow-dirty` or `--allow-no-vcs` to override.

Always review the diff before committing.

## Caveats around ambiguous diagnostics

The restore loop relies on `cargo check` diagnostics to figure out which
derives to put back. Some diagnostics — notably axum-style traits that use
`#[diagnostic::on_unimplemented]` to emit a custom message — don't pinpoint
the offending trait or sometimes even the offending type. Two failure modes
fall out of this:

1. **Item found, specific derive not pinpointable.** The diagnostic mentions
   the type but not which trait is missing. By default the tool restores
   *all* of that item's pending derives rather than guess, so a derive that
   was genuinely unused may remain in place ("over-restore").
2. **Item not found.** The diagnostic doesn't surface the type at all (or the
   tool can't match it to a known item by name, source-line peek, or span
   distance). In this case the tool can't restore anything for that
   diagnostic, and with `--write` you can be left with a project that no
   longer compiles.

Pass `--strict` to make both situations a hard failure instead, so you can
investigate and decide for yourself.

## How it works

1. Copy the project into a temporary sandbox so your sources are never touched
   directly.
2. Strip every `#[derive(...)]` attribute in the sandbox, leaving placeholder
   markers behind.
3. Run `cargo check` and iterate: parse diagnostics, restore the derives the
   compiler asks for, repeat until `cargo check` is clean.
4. If `--write` was passed, copy the modified files back to your project.

## License

MIT
