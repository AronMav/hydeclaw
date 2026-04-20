//! Codegen binary: generates `ui/src/types/api.generated.ts` from Rust DTOs.
//!
//! Run via: `cargo run --features ts-gen --bin gen_ts_types`
//! Or: `make gen-types` (from the workspace root)
//!
//! Uses ts-rs `T::decl(&Config::default())` to collect TypeScript declarations.
//! Add new types by: (1) annotating the struct, (2) exposing via dto_export in lib.rs,
//! (3) adding an import + collect_decl call below.

use hydeclaw_core::dto_export::{
    agents_dto::{
        AgentDetailAccessDto, AgentDetailApprovalDto, AgentDetailCompactionDto, AgentDetailDto,
        AgentDetailHeartbeatDto, AgentDetailHooksDto, AgentDetailRoutingDto, AgentDetailSessionDto,
        AgentDetailToolGroupsDto, AgentDetailToolLoopDto, AgentDetailToolsDto,
        AgentDetailWatchdogDto,
        AgentInfoDto, AgentInfoToolPolicyDto,
    },
    github_dto::GitHubRepo,
    AllowlistEntry,
    Notification, NotificationsResponseDto,
    Session, MessageRow,
    channels_dto::{ChannelRowDto, ActiveChannelDto},
    cron_dto::{CronJobDto, CronRunDto},
};
use ts_rs::TS;

/// Returns the `export type Foo = ...` declaration string for type T.
fn collect_decl<T: TS>() -> String {
    format!("export {}", T::decl(&ts_rs::Config::default()))
}

fn main() {
    let decls: Vec<String> = vec![
        // Phase B: AgentDetail DTO tree — nested types first, top-level last.
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
        // Phase C: DB-layer typed structs.
        collect_decl::<GitHubRepo>(),
        collect_decl::<AllowlistEntry>(),
        // Phase A Wave 1: AgentInfo DTO tree — nested type first.
        collect_decl::<AgentInfoToolPolicyDto>(),
        collect_decl::<AgentInfoDto>(),
        // Phase A Wave 1: DB notification types.
        collect_decl::<Notification>(),
        collect_decl::<NotificationsResponseDto>(),
        // Phase A Wave 1: DB session + message types.
        collect_decl::<Session>(),
        collect_decl::<MessageRow>(),
        // Phase A Wave 2: Channel DTOs.
        collect_decl::<ChannelRowDto>(),
        collect_decl::<ActiveChannelDto>(),
        // Phase A Wave 2: Cron DTOs.
        collect_decl::<CronJobDto>(),
        collect_decl::<CronRunDto>(),
    ];

    let header = "// @generated — do not edit by hand.\n\
// Source of truth: crates/hydeclaw-core/src/gateway/handlers/agents/dto_structs.rs (Phase B),\n\
//                  crates/hydeclaw-core/src/db/github.rs + approvals.rs (Phase C),\n\
//                  crates/hydeclaw-core/src/db/notifications.rs + sessions.rs (Phase A W1)\n\
//                  crates/hydeclaw-core/src/gateway/handlers/channels_dto_structs.rs (Phase A W2)\n\
//                  crates/hydeclaw-core/src/gateway/handlers/cron_dto_structs.rs (Phase A W2)\n\
// Regenerate with: make gen-types\n\n";

    let body = decls.join("\n\n");
    let output = format!("{}{}\n", header, body);

    let out_path = std::path::Path::new("ui/src/types/api.generated.ts");
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
