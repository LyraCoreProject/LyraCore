# Private integration fixtures

`Standalone` owns an isolated SpacetimeDB process, CLI configuration and database storage. Tests
can publish the selected Core Module, call reducers, read durable state and retain failure logs.
`module_bytes()` and `gateway_binary()` build the tested artifacts once per test process.

Core tests use this crate's workspace. Package test runners set `LYRACORE_TEST_CORE` to the explicit
Core checkout used for both builds and evidence. Package-specific setup and assertions belong in
the Package repository.

The Gateway coordinator also supports Package-owned tests of its private routing operations. A
test runner can set `LYRACORE_COORDINATOR_TEST_SOURCE` to a Rust source file. Cargo compiles that
source under `stdb::subscriptions::package_tests` only in test builds. Normal Core tests need no
Package source, and production Gateway builds do not compile these Package tests.
