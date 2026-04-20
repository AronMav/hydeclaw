# ts-rs Type Mapping Reference

**Context:** `ts-rs 12` with features `chrono-impl`, `uuid-impl`, `serde-json-impl`.
Applies to: `crates/hydeclaw-core`, `crates/hydeclaw-types`.

## Type mappings

| Rust type | TypeScript | Feature / Override |
|---|---|---|
| `String` | `string` | — |
| `bool` | `boolean` | — |
| `u8`, `u16`, `u32`, `i8`, `i16`, `i32`, `f32`, `f64` | `number` | — |
| `u64`, `i64`, `u128`, `i128` | `bigint` | **Override with `#[cfg_attr(feature = "ts-gen", ts(type = "number"))]` for token counts, timestamps-as-int, sizes that fit in JS safe integer range** |
| `usize`, `isize` | `number` | — |
| `Option<T>` | `T \| null` | Use `#[cfg_attr(feature = "ts-gen", ts(optional))]` for fields with `#[serde(skip_serializing_if = "Option::is_none")]` to get `T?` instead of `T \| null` |
| `Vec<T>` | `Array<T>` | — |
| `HashMap<String, V>` | `Record<string, V>` | — |
| `Uuid` | `string` | via `uuid-impl` feature |
| `DateTime<Utc>` | `string` | via `chrono-impl` feature (ISO 8601) |
| `serde_json::Value` | `unknown` | via `serde-json-impl` feature. Default is `unknown` in ts-rs 12. Use `#[cfg_attr(feature = "ts-gen", ts(type = "any"))]` per-field if `any` is needed. |
| `#[serde(rename = "foo")]` field | `foo:` in TS | ts-rs respects serde rename |
| `#[serde(rename_all = "camelCase")]` struct | camelCase keys in TS | ts-rs respects serde rename_all |

## Adding a new type to codegen

1. **Annotate the struct** in its source file:
   ```rust
   #[derive(Debug, Serialize)]
   #[cfg_attr(feature = "ts-gen", derive(ts_rs::TS))]
   #[cfg_attr(feature = "ts-gen", ts(export))]
   pub struct MyNewDto { ... }
   ```

2. **Expose via `dto_export` in `lib.rs`** (if the struct's module isn't already in lib.rs):
   ```rust
   // In the #[cfg(feature = "ts-gen")] pub mod dto_export { ... } block:
   #[path = "../path/to/module.rs"]
   pub mod my_module_dto;
   ```
   If the struct is already in an always-on lib module (e.g. `db::approvals`), just re-export:
   ```rust
   pub use crate::db::approvals::MyStruct;
   ```

3. **Register in `gen_ts_types.rs`**:
   ```rust
   use hydeclaw_core::dto_export::my_module_dto::MyNewDto;
   // ...
   collect_decl::<MyNewDto>(),
   ```

4. **Regenerate**: `make gen-types`

5. **Update `api.ts`** if a manual interface exists for this type:
   ```typescript
   // Replace:
   export interface MyInterface { ... }
   // With:
   export type { MyNewDto as MyInterface } from "./api.generated";
   ```

6. **Verify**: `cd ui && npm run build`

## Common mistakes

- **`u64` → bigint**: API consumers that expect `number` will silently receive `bigint`. Always add `#[ts(type = "number")]` for fields like `daily_budget_tokens`, `timeout_seconds`, `cooldown_secs`, `inactivity_secs`.
- **`Optional` vs nullable**: `Option<T>` without `skip_serializing_if` serializes as JSON `null` → TS `T | null`. With `skip_serializing_if = "Option::is_none"`, the field is absent from JSON → TS should be `T?`. Use `#[ts(optional)]` to express this.
- **Serde enums**: `#[serde(tag = "type")]`, `#[serde(untagged)]`, and `#[serde(flatten)]` are supported by ts-rs 12 but may produce unexpected shapes. Test with `collect_decl` before committing.
- **Path in dto_export**: `#[path]` in inline modules resolves relative to the module's virtual directory (`src/dto_export/`), not the file. Use `"../"` to navigate back to `src/`.
