# Binding schema parity

`gateway/tests/schema_parity.rs` checks the ordered names and types of every table in the
Coordinator subscription list. A Module column change without matching generated bindings
breaks BSATN row decoding. Regenerate bindings according to [danger zones](danger-zones.md).

The test builds native Module table types with `RawModuleDefV9Builder`, then resolves nested type
references with `Typespace::inline_typerefs_in_type`. This produces the same ordered product shape
used by schema extraction, without a running database.

Generated binding structs derive serialization and deserialization, but do not implement
`SpacetimeType`. The test constructs a real binding instance using its `Sentinel` trait and reads
two independent signals:

1. The derived `Debug` representation supplies field names in declaration order. A parser tracks
   brace, parenthesis and bracket depth so nested values cannot supply a top-level field name.
2. Field reads by name supply each real field type. Primitive and container fields use
   `SpacetimeType::make_type`; generated products read their fields, and generated sums check every
   variant and its BSATN tag. Module and SDK identity and timestamp types share the same pinned
   SpacetimeDB types.

The test compares field count, name at each index, and type at each index. Each manifest entry
names the Module type and the generated binding fields through `binding_shape!`.

Only Coordinator subscriptions are included. The test reads `stdb/connection.rs` to check manifest
completeness. Per-player subscriptions and reducer arguments are outside its scope.

The native Module dependency links on Linux, where the ELF linker removes unused Wasm host
intrinsics. The Gateway has no library target, so the test includes the self-contained generated
bindings tree through `#[path]` and compiles it into its own test binary.
