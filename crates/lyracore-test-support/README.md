# Private integration fixtures

`Standalone` owns an isolated SpacetimeDB process, CLI configuration and database storage. Tests
can publish the selected Core Module, call reducers, read durable state and retain failure logs.
`module_bytes()` builds the tested Module once per test process.
The test Module enables `debug_reducers` and `package_test_fixture`. The latter registers an inert
Package named `test_fixture` for Core's ownership and teardown tests; normal publishes omit it.

Core tests use this crate's workspace. Package test runners set `LYRACORE_TEST_CORE` to the explicit
Core checkout used for both builds and evidence. Package-specific setup and assertions belong in
the Package repository.
