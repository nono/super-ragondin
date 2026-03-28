# Proptest & Simulation Testing

Property-based testing with proptest and a custom simulator.

## Simulator Structure

- `crates/sync/src/simulator.rs` - Property-based testing simulator (with submodules)

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `PROPTEST_CASES` | `50` | Number of cases for proptest-based simulator tests (set to `500` in CI via `.github/workflows/ci.yml`) |

## Findings

- Proptest regression files (`*.proptest-regressions`) must be kept and checked into source control — they ensure known failure cases are always re-tested

## Debugging Proptest Failures

When a proptest failure occurs (in CI or locally), follow this workflow:

1. **Find the failing seed** in CI logs or `.proptest-regressions` files
2. **Add the regression case** and confirm it reproduces locally
3. **Diagnose the root cause** — check for MULTIPLE related bugs
4. **Fix and verify:**
   ```bash
   PROPTEST_CASES=100 cargo test <test_name>
   ```
5. **Run the full suite:**
   ```bash
   cargo test --all
   ```
6. **Commit** with message: `fix: <description of proptest bug>`

## References

- [proptest - Hypothesis-like property testing for Rust](https://github.com/proptest-rs/proptest)
