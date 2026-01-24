use std::path::Path;
use std::sync::Mutex;

use tempfile::TempDir;

use kagent::skill::{
    Skill, SkillType, discover_skills, discover_skills_from_roots, find_user_skills_dir,
    get_builtin_skills_dir, resolve_skills_roots,
};
use kaos::KaosPath;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: tests serialize env access via ENV_LOCK to avoid races.
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(prev) = &self.prev {
            // SAFETY: tests serialize env access via ENV_LOCK to avoid races.
            unsafe {
                std::env::set_var(self.key, prev);
            }
        } else {
            // SAFETY: tests serialize env access via ENV_LOCK to avoid races.
            unsafe {
                std::env::remove_var(self.key);
            }
        }
    }
}

fn write_skill(skill_dir: &Path, content: &str) {
    std::fs::create_dir_all(skill_dir).expect("create skill dir");
    std::fs::write(skill_dir.join("SKILL.md"), content).expect("write skill");
}

#[tokio::test]
async fn test_discover_skills_parses_frontmatter_and_defaults() {
    let root = TempDir::new().expect("temp dir");
    let root_path = root.path().join("skills");
    std::fs::create_dir_all(&root_path).expect("create skills root");

    write_skill(
        &root_path.join("alpha"),
        "---\nname: alpha-skill\ndescription: Alpha description\n---\n",
    );
    write_skill(&root_path.join("beta"), "# No frontmatter");

    let root_path = KaosPath::unsafe_from_local_path(&root_path);
    let mut skills = discover_skills(&root_path).await;
    let base_dir = KaosPath::unsafe_from_local_path(Path::new("/path/to"));
    for skill in &mut skills {
        let relative_dir = skill.dir.relative_to(&root_path).expect("relative");
        skill.dir = base_dir.clone() / &relative_dir;
    }

    assert_eq!(
        skills,
        vec![
            Skill {
                name: "alpha-skill".to_string(),
                description: "Alpha description".to_string(),
                skill_type: SkillType::Standard,
                dir: KaosPath::unsafe_from_local_path(Path::new("/path/to/alpha")),
                flow: None,
            },
            Skill {
                name: "beta".to_string(),
                description: "No description provided.".to_string(),
                skill_type: SkillType::Standard,
                dir: KaosPath::unsafe_from_local_path(Path::new("/path/to/beta")),
                flow: None,
            },
        ]
    );
}

#[tokio::test]
async fn test_discover_skills_parses_flow_type() {
    let root = TempDir::new().expect("temp dir");
    let root_path = root.path().join("skills");
    std::fs::create_dir_all(&root_path).expect("create skills root");

    write_skill(
        &root_path.join("flowy"),
        "---\nname: flowy\ndescription: Flow skill\ntype: flow\n---\n```mermaid\nflowchart TD\nBEGIN([BEGIN]) --> A[Hello]\nA --> END([END])\n```\n",
    );

    let skills = discover_skills(&KaosPath::unsafe_from_local_path(&root_path)).await;

    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].skill_type, SkillType::Flow);
    assert!(skills[0].flow.is_some());
    assert_eq!(skills[0].flow.as_ref().unwrap().begin_id, "BEGIN");
}

#[tokio::test]
async fn test_discover_skills_flow_parse_failure_falls_back() {
    let root = TempDir::new().expect("temp dir");
    let root_path = root.path().join("skills");
    std::fs::create_dir_all(&root_path).expect("create skills root");

    write_skill(
        &root_path.join("broken-flow"),
        "---\nname: broken-flow\ndescription: Broken flow skill\ntype: flow\n---\n```mermaid\nflowchart TD\nA --> B\n```\n",
    );

    let skills = discover_skills(&KaosPath::unsafe_from_local_path(&root_path)).await;

    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].skill_type, SkillType::Standard);
    assert!(skills[0].flow.is_none());
}

#[tokio::test]
async fn test_discover_skills_from_roots_prefers_later_dirs() {
    let root = TempDir::new().expect("temp dir");
    let root_path = root.path().join("root");
    let system_dir = root_path.join("system");
    let user_dir = root_path.join("user");
    std::fs::create_dir_all(&system_dir).expect("create system dir");
    std::fs::create_dir_all(&user_dir).expect("create user dir");

    write_skill(
        &system_dir.join("shared"),
        "---\nname: shared\ndescription: System version\n---\n",
    );
    write_skill(
        &user_dir.join("shared"),
        "---\nname: shared\ndescription: User version\n---\n",
    );

    let root_path = KaosPath::unsafe_from_local_path(&root_path);
    let mut skills = discover_skills_from_roots(&[
        KaosPath::unsafe_from_local_path(&system_dir),
        KaosPath::unsafe_from_local_path(&user_dir),
    ])
    .await;
    let base_dir = KaosPath::unsafe_from_local_path(Path::new("/path/to"));
    for skill in &mut skills {
        let relative_dir = skill.dir.relative_to(&root_path).expect("relative");
        skill.dir = base_dir.clone() / &relative_dir;
    }

    assert_eq!(
        skills,
        vec![Skill {
            name: "shared".to_string(),
            description: "User version".to_string(),
            skill_type: SkillType::Standard,
            dir: KaosPath::unsafe_from_local_path(Path::new("/path/to/user/shared")),
            flow: None,
        }]
    );
}

#[tokio::test]
async fn test_resolve_skills_roots_uses_layers() {
    let _lock = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().expect("temp dir");
    let home_dir = tmp.path().join("home");
    let user_dir = home_dir.join(".config/agents/skills");
    std::fs::create_dir_all(&user_dir).expect("create user skills dir");
    let _home_guard = EnvGuard::set("HOME", home_dir.to_str().expect("home"));
    let _profile_guard = EnvGuard::set("USERPROFILE", home_dir.to_str().expect("home"));

    let work_dir = tmp.path().join("project");
    let project_dir = work_dir.join(".agents/skills");
    std::fs::create_dir_all(&project_dir).expect("create project skills dir");

    let roots = resolve_skills_roots(&KaosPath::unsafe_from_local_path(&work_dir), None).await;

    assert_eq!(
        roots,
        vec![
            KaosPath::unsafe_from_local_path(&get_builtin_skills_dir()),
            KaosPath::unsafe_from_local_path(&user_dir),
            KaosPath::unsafe_from_local_path(&project_dir),
        ]
    );
}

#[tokio::test]
async fn test_resolve_skills_roots_respects_override() {
    let work_dir = TempDir::new().expect("temp dir");
    let override_dir = work_dir.path().join("override");
    std::fs::create_dir_all(&override_dir).expect("create override dir");

    let roots = resolve_skills_roots(
        &KaosPath::unsafe_from_local_path(work_dir.path()),
        Some(KaosPath::unsafe_from_local_path(&override_dir)),
    )
    .await;

    assert_eq!(
        roots,
        vec![
            KaosPath::unsafe_from_local_path(&get_builtin_skills_dir()),
            KaosPath::unsafe_from_local_path(&override_dir),
        ]
    );
}

#[tokio::test]
async fn test_find_user_skills_dir_uses_agents_candidate() {
    let _lock = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().expect("temp dir");
    let home_dir = tmp.path().join("home");
    let _home_guard = EnvGuard::set("HOME", home_dir.to_str().expect("home"));
    let _profile_guard = EnvGuard::set("USERPROFILE", home_dir.to_str().expect("home"));

    let agents_dir = home_dir.join(".agents/skills");
    std::fs::create_dir_all(&agents_dir).expect("create agents skills dir");

    let found = find_user_skills_dir().await.expect("user skills dir");
    assert_eq!(found, KaosPath::unsafe_from_local_path(&agents_dir));
}

#[tokio::test]
async fn test_find_user_skills_dir_uses_codex_candidate() {
    let _lock = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().expect("temp dir");
    let home_dir = tmp.path().join("home");
    let _home_guard = EnvGuard::set("HOME", home_dir.to_str().expect("home"));
    let _profile_guard = EnvGuard::set("USERPROFILE", home_dir.to_str().expect("home"));

    let codex_dir = home_dir.join(".codex/skills");
    std::fs::create_dir_all(&codex_dir).expect("create codex skills dir");

    let found = find_user_skills_dir().await.expect("user skills dir");
    assert_eq!(found, KaosPath::unsafe_from_local_path(&codex_dir));
}
