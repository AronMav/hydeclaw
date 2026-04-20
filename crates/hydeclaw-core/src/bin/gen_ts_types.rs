//! Codegen binary: generates `ui/src/types/api.generated.ts` from Rust DTOs.
//!
//! Run via: `cargo run --features ts-gen --bin gen_ts_types`
//! Or: `make gen-types` (from the workspace root)
//!
//! Uses ts-rs `T::decl(&Config::default())` to collect TypeScript declarations
//! for all 12 AgentDetail DTO structs and writes them into a single generated file.

use hydeclaw_core::dto_export::agents_dto::{
    AgentDetailAccessDto, AgentDetailApprovalDto, AgentDetailCompactionDto, AgentDetailDto,
    AgentDetailHeartbeatDto, AgentDetailHooksDto, AgentDetailRoutingDto, AgentDetailSessionDto,
    AgentDetailToolGroupsDto, AgentDetailToolLoopDto, AgentDetailToolsDto,
    AgentDetailWatchdogDto,
};
use ts_rs::TS;

/// Returns the `export type Foo = ...` declaration string for type T.
/// ts-rs `decl()` returns the declaration without the `export` keyword;
/// we prefix it here so the generated file has proper named exports.
fn collect_decl<T: TS>() -> String {
    format!("export {}", T::decl(&ts_rs::Config::default()))
}

fn main() {
    // Collect TypeScript declarations for all DTO types.
    // Order: nested types first, top-level last.
    let decls: Vec<String> = vec![
        collect_decl::<AgentDetailAccessDto>(),
        collect_decl::<AgentDetailHeartbeatDto>(),
        collect_decl::<AgentDetailToolGroupsDto>(),
        collect_decl::<AgentDetailToolsDto>(),
        collect_decl::<AgentDetailCompactionDto>(),
        collect_decl::<AgentDetailSessionDto>(),
        collect_decl::<AgentDetailToolLoopDto>(),
        collect_decl::<AgentDetailApprovalDto>(),
        collect_decl::<AgentDetailRoutingDto>(),
        collect_decl::<AgentDetailWatchdogDto>(),
        collect_decl::<AgentDetailHooksDto>(),
        collect_decl::<AgentDetailDto>(),
    ];

    let header = "// @generated — do not edit by hand.\n// Source of truth: crates/hydeclaw-core/src/gateway/handlers/agents/dto_structs.rs\n// Regenerate with: make gen-types\n\n";

    // ts-rs 12 decl() emits `type Foo = { ... };` (without `export`);
    // collect_decl() prefixes `export ` before joining.
    // Join all declarations separated by a blank line.
    let body = decls.join("\n\n");
    let output = format!("{}{}\n", header, body);

    // Determine output path relative to the workspace root.
    // The binary is expected to be run from the workspace root.
    let out_path = std::path::Path::new("ui/src/types/api.generated.ts");

    // Ensure the parent directory exists.
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("failed to create output dir {}: {e}", parent.display()));
    }

    std::fs::write(out_path, &output)
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", out_path.display()));

    println!(
        "Generated {} ({} types, {} bytes)",
        out_path.display(),
        decls.len(),
        output.len()
    );
}
