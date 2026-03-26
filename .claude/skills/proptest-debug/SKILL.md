# Proptest Debug Skill
1. Find the failing seed in CI logs or .proptest-regressions files
2. Add the regression case and confirm it reproduces locally
3. Diagnose the root cause - check for MULTIPLE related bugs
4. Fix and run: PROPTEST_CASES=100 cargo test <test_name>
5. Run full suite: cargo test --all
6. Commit with message: fix: <description of proptest bug>
