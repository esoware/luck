# luck_resolver rewrite report

Recovered from the rewrite agent's transcript (its structured-output call
failed; the work itself completed and was independently verified).

## Diagnosis

luck_resolver maps a require string to a file path for Lua template paths
and Luau relative/alias imports. The job is small and well-defined but the
surface was awkward and carried a hidden-state hazard. Concrete defects:
(1) resolve() was a free function with six positional args; three adjacent
string/path-ish params (type-swap hazard) and search_paths/rc_dir dead for
the Luau branch - one signature over two disjoint parameter sets.
(2) Spans travelled as std::ops::Range<usize>, forcing require_span.clone()
at nearly every diagnostic site, against the workspace rule that spans stay
luck_token::Span until the single conversion point at the diagnostic
boundary. (3) A thread-local mutable .luaurc cache plus a public
clear_luaurc_cache() escape hatch - the cache persisted across builds on a
thread, so alias edits were invisible until someone remembered to clear it;
a latent staleness bug patched by manual clear calls in luck_cli watch mode
and the LSP. (4) A crate-level allow(clippy::result_large_err) papering
over the large Err type. (5) Two inline literal E004 codes hand-built in
luau.rs, bypassing the centralized errors:: constructors. (6) ResolveResult
(Result-in-a-name), duplicated per-extension probing boilerplate, and
tests named test_* by code path each threading a literal 0..10 span.

## What changed

Replaced the free resolve() + global cache with a Resolver struct that owns
its .luaurc cache, a borrow-only ResolveRequest<'a> (named fields: module,
from_file, target, search_paths, project_root, span) and a renamed
ResolvedModule result. The cache is now an owned value whose lifetime is a
build - the bundler makes one per build_graph, so alias edits are always
seen and clear_luaurc_cache() is deleted. Strictly better than the old
thread_local under rayon: distinct builds hold distinct resolvers, no
shared RefCell, no cross-build staleness.

Spans enter as luck_token::Span and convert to Range only at diagnostic
construction; every require_span.clone() disappeared.

The Luau resolver became an impl Resolver block: resolve_luau ->
resolve_relative / resolve_alias / resolve_self, with luaurc_chain and
luaurc_aliases as the two cache methods. Two near-identical chain consumers
collapsed into one luaurc_aliases; the @self shadow (W004) check is now
aliases.contains_key(self). Extracted append_extension(), os_path(), and
ambiguous() helpers, killing repeated OsString/slash-translation
boilerplate.

Boxed the error: resolve returns Result<ResolvedModule, Box<Diagnostic>>,
removing the result_large_err allow instead of suppressing it. Moved the
two inline E004 diagnostics into new centralized luck_core constructors
errors::e004_luau_scheme and errors::e004_self_needs_subpath.

Rewrote the tests with scenario names (resolves_module_from_first_template,
flags_ambiguous_extension, closest_luaurc_wins_over_ancestor, ...), each
routed through a single resolve_lua_module / resolve_luau_module helper;
all original assertions preserved.

## Cross-crate fallout

- luck_resolver public API: removed pub fn resolve, pub struct
  ResolveResult, pub fn clear_luaurc_cache; added Resolver, ResolveRequest,
  ResolvedModule. normalize_path unchanged.
- luck_bundler graph.rs: build_graph creates one Resolver::new() and
  threads &mut resolver into process_module; Err arm unboxes
  Box<Diagnostic>.
- luck_bundler require_extraction.rs: RequireInfo.span changed from
  Range<usize> to Span (also removed a redundant duplicate field);
  call_span stays Range for bundler-side cycle diagnostics.
- luck_cli: removed the clear_luaurc_cache() call in
  run_build_collect_paths; dropped the now-unused luck_resolver dep.
- luck_lsp: removed the clear_luaurc_cache() call in
  did_change_watched_files; dropped the now-unused luck_resolver dep;
  corrected a stale doc comment in providers/document_link.rs.
- luck_core diagnostics.rs: additive errors::e004_luau_scheme and
  errors::e004_self_needs_subpath; no existing signatures changed.
- luck facade re-exports the module wholesale, so no per-item fix needed.

## Docs updated

- crates/luck_resolver/README.md (API section for the new surface and the
  owned-cache model).
- crates/luck_resolver/src/lib.rs module docs + Usage doctest.
- crates/luck_lsp/src/providers/document_link.rs doc comment.
- CLAUDE.md resolver row still accurate; no skill references the removed
  API.

## Flags

- Deliberate deviation: Result<_, Box<Diagnostic>> diverges from the
  unboxed-Diagnostic house convention; boxed to remove result_large_err
  per the standard. One *diag unbox at the single call site.
- luck_lsp providers/document_link.rs implements an ad-hoc require
  resolver that does NOT match luck_resolver semantics (ignores @aliases
  and the init parent-parent rule). Doc comment fixed; unification left
  for the lsp crate agent.
- luck_bundler process_module carries 13 args behind
  allow(clippy::too_many_arguments) (now 14 with the resolver); wants a
  context struct - bundler agent's job. Its result_large_err allow is
  likely removable too.
- Left alone with reason: normalize_path returns String because the
  bundler uses it as the module graph's canonical key and for
  cross-platform-stable display; PathBuf would fight that identity model.
- No behavior bugs found; all observable outputs (E004/E007/W004, extension
  and init preference, closest-.luaurc-wins, @self parent rule) preserved
  and still asserted.

## Gates

cargo build --workspace: clean. cargo clippy --workspace --all-targets --
-D warnings: clean. cargo nextest run --workspace: 2000 passed, 4 skipped
(matches baseline). Per-crate nextest (resolver, bundler, cli, core): 191
passed, 1 skipped. Doctests pass. Bundler bench vs pre-rewrite baseline:
4.257 ms vs 4.222 ms, within noise. Minifier/codegen untouched; minsize
unaffected.

Independently re-verified after recovery: clippy clean, 2000/2000 tests,
all 17 doctest suites pass.
