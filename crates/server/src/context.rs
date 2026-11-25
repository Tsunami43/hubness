//! The session system prompt — a port of `context.ts`.
//!
//! Levels, from general to specific: hub-wide session instructions → group → project →
//! session. A more specific level wins on conflict. AGENTS.md / CLAUDE.md in repositories
//! are read by the agents themselves.
//!
//! TODO(rust-port): the `.hub/hub.yaml` manifest section (`manifestPrompt`).

use hubness_core::{GitProviderId, Project, ProjectGroup};

fn section(title: &str, body: &str) -> String {
    let body = body.trim();
    if body.is_empty() { String::new() } else { format!("## {title}\n{body}") }
}

fn join_non_empty(parts: Vec<String>, sep: &str) -> String {
    parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(sep)
}

/// Hub-wide instructions first, then the scope context. `None` — nothing to add.
pub fn session_system_prompt(hub_instructions: &str, scope: Option<String>) -> Option<String> {
    let s = join_non_empty(vec![section("General instructions", hub_instructions), scope.unwrap_or_default()], "\n\n");
    (!s.is_empty()).then_some(s)
}

/// Project session context: description and instructions. `None` — nothing to add.
pub fn project_context(p: &Project) -> Option<String> {
    let s = join_non_empty(vec![section("Project", &p.description), section("Project instructions", &p.instructions)], "\n\n");
    (!s.is_empty()).then_some(s)
}

fn provider_label(p: Option<GitProviderId>) -> &'static str {
    match p {
        Some(GitProviderId::Github) => "GitHub",
        Some(GitProviderId::Gitlab) => "GitLab",
        Some(GitProviderId::Bitbucket) => "Bitbucket",
        _ => "Git",
    }
}

fn repo_line(p: &Project) -> String {
    let mut bits = vec![format!("path: {}", p.path)];
    if let Some(b) = &p.default_branch {
        bits.push(format!("default branch: {b}"));
    }
    if let Some(url) = &p.repo_url {
        bits.push(format!("remote: {url} ({})", provider_label(p.git_provider)));
    }
    format!("- {} — {}", p.name, bits.join(", "))
}

/// Aggregated group session context: what the product is, which repositories it includes,
/// and the description and instructions of each (repository info from the project cache).
pub fn group_context(g: &ProjectGroup, repos: &[Project]) -> String {
    let header = [
        format!("You are working on \"{}\" — a group of {} repositories that make up one product.", g.name, repos.len()),
        "Your working directory contains a link to each repository; the real paths are listed below and are all available to you.".into(),
        "Only these repositories belong to this session. Instructions closer to a repository override the group-wide ones when they conflict."
            .into(),
    ]
    .join(" ");
    let per_repo = join_non_empty(
        repos
            .iter()
            .map(|p| {
                let instr = if p.instructions.is_empty() { String::new() } else { format!("Instructions: {}", p.instructions) };
                let extra = join_non_empty(vec![p.description.clone(), instr], "\n");
                if extra.is_empty() { String::new() } else { format!("### {}\n{extra}", p.name) }
            })
            .collect(),
        "\n\n",
    );
    join_non_empty(
        vec![
            header,
            section("Group", &g.description),
            section("Group instructions", &g.instructions),
            section("Repositories", &repos.iter().map(repo_line).collect::<Vec<_>>().join("\n")),
            if per_repo.is_empty() { String::new() } else { section("Repository notes", &per_repo) },
        ],
        "\n\n",
    )
}
