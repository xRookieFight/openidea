//! Which IntelliJ run configuration type each OpenIDEA kind is, and how the
//! kind's form params map onto that type's options. Anything a type can't hold
//! falls through to the sidecar (see `super::sidecar`), so a mapping here only
//! has to be right, not complete.

use super::{from_idea, to_idea};
use crate::RunConfig;
use ide_core::idea_xml::Element;
use std::path::Path;

/// Kinds IntelliJ has no type for are stored as this, with the kind as factory.
/// IntelliJ shows them as unknown configurations and leaves them alone.
pub(crate) const FALLBACK: &str = "OpenIdeaRunConfiguration";

pub(crate) const COMPOUND: &str = "compound";

const TABLE: &[(&str, &str, Option<&str>)] = &[
    ("shell", "ShConfigurationType", None),
    ("npm", "js.build_tools.npm", None),
    ("gradle", "GradleRunConfiguration", Some("Gradle")),
    ("maven", "MavenRunConfiguration", Some("Maven")),
    ("application", "Application", Some("Application")),
    ("spring-boot", "SpringBootApplicationConfigurationType", Some("Spring Boot")),
    (COMPOUND, "CompoundRunConfigurationType", None),
    ("remote-jvm", "Remote", Some("Remote")),
    ("python-remote", "PyRemoteDebugConfigurationType", Some("Python Remote Debug")),
    ("node-remote", "ChromiumRemoteDebugType", Some("Attach to Node.js/Chrome")),
    ("go-remote", "GoRemoteDebugConfigurationType", Some("Go Remote")),
    ("docker-dockerfile", "docker-deploy", Some("dockerfile")),
    ("docker-image", "docker-deploy", Some("docker-image")),
    ("docker-compose", "docker-deploy", Some("docker-compose.yml")),
];

/// IntelliJ `type` and `factoryName` for `kind`.
pub(crate) fn idea_type(kind: &str) -> (&str, Option<&str>) {
    match TABLE.iter().find(|(k, ..)| *k == kind) {
        Some((_, ty, factory)) => (ty, *factory),
        None => (FALLBACK, Some(kind)),
    }
}

/// The kind of an IntelliJ configuration; `None` for types OpenIDEA doesn't know.
pub(crate) fn kind_of(ty: &str, factory: Option<&str>) -> Option<String> {
    if ty == FALLBACK {
        return factory.filter(|f| !f.is_empty()).map(str::to_owned);
    }
    TABLE
        .iter()
        // docker-deploy is one type with three factories; the others only look at the type
        .find(|(_, t, f)| *t == ty && (ty != "docker-deploy" || *f == factory))
        .map(|(k, ..)| (*k).to_owned())
}

/// Types whose configurations keep their environment in `<envs>`.
pub(crate) fn has_envs(kind: &str) -> bool {
    matches!(kind, "shell" | "npm" | "application" | "spring-boot") || idea_type(kind).0 == FALLBACK
}

fn p<'a>(c: &'a RunConfig, key: &str) -> &'a str {
    c.extras.params.get(key).map(String::as_str).unwrap_or("")
}

fn non_empty(v: &str) -> Option<&str> {
    (!v.is_empty()).then_some(v)
}

/// Sets a param on read; empty IntelliJ values don't become params.
fn put(c: &mut RunConfig, key: &str, v: Option<&str>) {
    if let Some(v) = v.filter(|v| !v.is_empty()) {
        c.extras.params.insert(key.to_owned(), v.to_owned());
    }
}

fn value_child<'a>(e: &'a Element, name: &str) -> Option<&'a str> {
    e.child(name)?.attr("value")
}

fn set_value_child(e: &mut Element, name: &str, v: Option<&str>) {
    match v {
        Some(v) => e.child_or_insert(name).set_attr("value", v),
        None => e.retain_elements(|c| c.name != name),
    }
}

fn split_words(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_owned).collect()
}

/// A command line as one shell string, quoting the args that need it.
fn shell_line(command: &str, args: &[String]) -> String {
    let quote = |a: &str| {
        if !a.is_empty() && !a.contains(|c: char| c.is_whitespace() || "'\"\\$`".contains(c)) {
            a.to_owned()
        } else {
            format!("'{}'", a.replace('\'', r"'\''"))
        }
    };
    std::iter::once(command.to_owned()).chain(args.iter().map(|a| quote(a))).collect::<Vec<_>>().join(" ")
}

fn cwd_of(c: &RunConfig) -> Option<String> {
    c.cwd.as_ref().map(|p| p.to_string_lossy().into_owned())
}

fn set_cwd(c: &mut RunConfig, root: &Path, v: Option<&str>) {
    let rel = v.map(|v| from_idea(root, v)).unwrap_or_default();
    c.cwd = (!rel.is_empty()).then(|| std::path::PathBuf::from(rel).into());
}

/// Writes the type's own options into `e`. `fresh`: the element is new, so the
/// defaults IntelliJ always writes for the type go in too.
pub(crate) fn write(kind: &str, c: &RunConfig, e: &mut Element, root: &Path, fresh: bool) {
    match kind {
        "shell" => write_shell(c, e, root),
        "npm" => {
            let dir = p(c, "packageDir");
            let pkg = if dir.is_empty() { "package.json".to_owned() } else { format!("{dir}/package.json") };
            set_value_child(e, "package-json", Some(&to_idea(root, &pkg)));
            set_value_child(e, "command", Some(non_empty(p(c, "command")).unwrap_or("run")));
            match non_empty(p(c, "script")) {
                Some(s) => {
                    let scripts = e.child_or_insert("scripts");
                    scripts.children.clear();
                    scripts.push(Element::new("script").with_attr("value", s));
                }
                None => e.retain_elements(|x| x.name != "scripts"),
            }
            set_value_child(e, "arguments", non_empty(p(c, "arguments")));
            if e.child("node-interpreter").is_none() {
                set_value_child(e, "node-interpreter", Some("project"));
            }
            set_value_child(e, "package-manager", non_empty(p(c, "packageManager")));
        }
        "gradle" => {
            let s = e.child_or_insert("ExternalSystemSettings");
            s.set_option("externalProjectPath", Some(&to_idea(root, p(c, "projectDir"))));
            s.set_option("externalSystemIdString", Some("GRADLE"));
            s.set_option("scriptParameters", Some(p(c, "arguments")));
            if s.option_node("taskDescriptions").is_none() {
                s.set_option_list("taskDescriptions", &[]);
            }
            s.set_option_list("taskNames", &split_words(p(c, "tasks")));
            s.set_option("vmOptions", non_empty(p(c, "vmOptions")));
            if fresh {
                for (name, v) in [
                    ("ExternalSystemDebugServerProcess", "true"),
                    ("ExternalSystemReattachDebugProcess", "true"),
                    ("DebugAllEnabled", "false"),
                    ("RunAsTest", "false"),
                ] {
                    let mut flag = Element::new(name);
                    flag.set_text(v);
                    e.push(flag);
                }
            }
        }
        "maven" => {
            let settings = e.child_or_insert("MavenSettings");
            let params = settings.option_node_mut("myRunnerParameters").child_or_insert("MavenRunnerParameters");
            params.set_option("cmdOptions", non_empty(p(c, "arguments")));
            params.set_option_list("goals", &split_words(p(c, "tasks")));
            params.set_option("workingDirPath", Some(&to_idea(root, p(c, "projectDir"))));
            match non_empty(p(c, "vmOptions")) {
                Some(vm) => settings
                    .option_node_mut("myRunnerSettings")
                    .child_or_insert("MavenRunnerSettings")
                    .set_option("vmOptions", Some(vm)),
                None => {
                    if let Some(s) = settings.option_node_mut("myRunnerSettings").child_mut("MavenRunnerSettings") {
                        s.set_option("vmOptions", None);
                    }
                }
            }
        }
        "application" | "spring-boot" => {
            let spring = kind == "spring-boot";
            e.set_option(if spring { "SPRING_BOOT_MAIN_CLASS" } else { "MAIN_CLASS_NAME" }, Some(p(c, "mainClass")));
            if spring {
                e.set_option("ACTIVE_PROFILES", non_empty(p(c, "activeProfiles")));
            }
            e.set_option("PROGRAM_PARAMETERS", non_empty(p(c, "programArgs")));
            e.set_option("VM_PARAMETERS", non_empty(p(c, "vmOptions")));
            e.set_option("WORKING_DIRECTORY", cwd_of(c).map(|d| to_idea(root, &d)).as_deref());
            e.set_option("PASS_PARENT_ENVS", (!c.extras.include_system_env).then_some("false"));
        }
        "remote-jvm" => {
            e.set_option("USE_SOCKET_TRANSPORT", Some("true"));
            e.set_option("SERVER_MODE", Some(if p(c, "mode") == "listen" { "true" } else { "false" }));
            if fresh {
                e.set_option("SHMEM_ADDRESS", Some(""));
            }
            e.set_option("HOST", Some(p(c, "host")));
            e.set_option("PORT", Some(p(c, "port")));
            if fresh {
                e.set_option("AUTO_RESTART", Some("false"));
            }
        }
        "python-remote" => {
            e.set_option("PORT", Some(p(c, "port")));
            e.set_option("HOST", Some(p(c, "host")));
            let local = p(c, "localRoot");
            let remote = p(c, "remoteRoot");
            if local.is_empty() && remote.is_empty() {
                e.retain_elements(|x| x.name != "PathMappingSettings");
            } else {
                let list =
                    e.child_or_insert("PathMappingSettings").option_node_mut("pathMappings").child_or_insert("list");
                list.children.clear();
                list.push(
                    Element::new("mapping")
                        .with_attr("local-root", to_idea(root, local))
                        .with_attr("remote-root", remote),
                );
            }
        }
        "node-remote" | "go-remote" => {
            e.set_attr("host", p(c, "host"));
            e.set_attr("port", p(c, "port"));
            if kind == "go-remote" {
                if fresh {
                    e.set_option("disconnectOption", Some("ASK"));
                }
            } else {
                let remote = p(c, "remoteRoot");
                e.retain_elements(|x| x.name != "mapping");
                if !remote.is_empty() {
                    e.push(
                        Element::new("mapping")
                            .with_attr("url", remote)
                            .with_attr("local-file", to_idea(root, p(c, "localRoot"))),
                    );
                }
            }
        }
        "docker-dockerfile" | "docker-image" | "docker-compose" => {
            match non_empty(p(c, "connection")) {
                Some(server) => e.set_attr("server-name", server),
                None => e.remove_attr("server-name"),
            }
            let (_, factory) = idea_type(kind);
            let deployment = e.child_or_insert("deployment");
            deployment.set_attr("type", factory.unwrap_or_default());
            let s = deployment.child_or_insert("settings");
            match kind {
                "docker-dockerfile" => {
                    s.set_option("imageTag", non_empty(p(c, "tag")));
                    s.set_option("containerName", non_empty(p(c, "containerName")));
                    s.set_option("contextFolderPath", non_empty(p(c, "context")).map(|d| to_idea(root, d)).as_deref());
                    s.set_option("sourceFilePath", non_empty(p(c, "dockerfile")).map(|f| to_idea(root, f)).as_deref());
                }
                "docker-image" => {
                    s.set_option("imageTag", non_empty(p(c, "image")));
                    s.set_option("containerName", non_empty(p(c, "containerName")));
                }
                _ => {
                    let first = p(c, "files").split([',', ';', '\n']).map(str::trim).find(|f| !f.is_empty());
                    s.set_option("sourceFilePath", first.map(|f| to_idea(root, f)).as_deref());
                }
            }
        }
        COMPOUND => {}
        _ => {
            e.set_option("command", Some(&c.command));
            e.set_option_list("args", &c.args);
            e.set_option("cwd", cwd_of(c).map(|d| to_idea(root, &d)).as_deref());
            let params = e.child_or_insert("params");
            params.children.clear();
            for (k, v) in &c.extras.params {
                params.push(Element::new("param").with_attr("name", k.as_str()).with_attr("value", v.as_str()));
            }
        }
    }
}

fn write_shell(c: &RunConfig, e: &mut Element, root: &Path) {
    let mode = non_empty(p(c, "mode")).unwrap_or("command");
    let text = match mode {
        "text" => p(c, "script").to_owned(),
        "command" if c.extras.params.contains_key("command") => {
            format!("{} {}", p(c, "command"), p(c, "args")).trim().to_owned()
        }
        "command" => shell_line(&c.command, &c.args),
        _ => String::new(),
    };
    let file = mode == "file";
    e.set_option("SCRIPT_TEXT", Some(&text));
    e.set_option("INDEPENDENT_SCRIPT_PATH", Some("true"));
    e.set_option("SCRIPT_PATH", Some(&if file { to_idea(root, p(c, "path")) } else { String::new() }));
    e.set_option("SCRIPT_OPTIONS", Some(if file { p(c, "options") } else { "" }));
    e.set_option("INDEPENDENT_SCRIPT_WORKING_DIRECTORY", Some("true"));
    e.set_option("SCRIPT_WORKING_DIRECTORY", Some(&to_idea(root, &cwd_of(c).unwrap_or_default())));
    e.set_option("INDEPENDENT_INTERPRETER_PATH", Some("true"));
    e.set_option("INTERPRETER_PATH", Some(p(c, "interpreter")));
    e.set_option("INTERPRETER_OPTIONS", Some(p(c, "interpreterOptions")));
    e.set_option("EXECUTE_IN_TERMINAL", Some(if c.extras.emulate_terminal { "true" } else { "false" }));
    e.set_option("EXECUTE_SCRIPT_FILE", Some(if file { "true" } else { "false" }));
}

/// Reads the type's own options from `e` into `c` (params, and the few
/// top-level fields a type stores itself, like the working directory).
pub(crate) fn read(kind: &str, e: &Element, root: &Path, c: &mut RunConfig) {
    let rel = |v: Option<&str>| v.map(|v| from_idea(root, v));
    match kind {
        "shell" => {
            if e.option("EXECUTE_SCRIPT_FILE") == Some("true") {
                put(c, "mode", Some("file"));
                put(c, "path", rel(e.option("SCRIPT_PATH")).as_deref());
                put(c, "options", e.option("SCRIPT_OPTIONS"));
            } else {
                put(c, "mode", Some("text"));
                put(c, "script", e.option("SCRIPT_TEXT"));
            }
            put(c, "interpreter", e.option("INTERPRETER_PATH"));
            put(c, "interpreterOptions", e.option("INTERPRETER_OPTIONS"));
            set_cwd(c, root, e.option("SCRIPT_WORKING_DIRECTORY"));
            c.extras.emulate_terminal = e.option("EXECUTE_IN_TERMINAL") == Some("true");
        }
        "npm" => {
            let pkg = rel(value_child(e, "package-json")).unwrap_or_default();
            let dir = pkg.strip_suffix("package.json").unwrap_or(&pkg).trim_end_matches('/');
            put(c, "packageDir", Some(dir));
            put(c, "command", value_child(e, "command"));
            put(c, "script", e.child("scripts").and_then(|s| value_child(s, "script")));
            put(c, "arguments", value_child(e, "arguments"));
            put(c, "packageManager", value_child(e, "package-manager"));
        }
        "gradle" => {
            if let Some(s) = e.child("ExternalSystemSettings") {
                put(c, "projectDir", rel(s.option("externalProjectPath")).as_deref());
                put(c, "tasks", Some(&s.option_list("taskNames").join(" ")));
                put(c, "arguments", s.option("scriptParameters"));
                put(c, "vmOptions", s.option("vmOptions"));
            }
        }
        "maven" => {
            let settings = e.child("MavenSettings");
            if let Some(params) = settings
                .and_then(|s| s.option_node("myRunnerParameters"))
                .and_then(|o| o.child("MavenRunnerParameters"))
            {
                put(c, "projectDir", rel(params.option("workingDirPath")).as_deref());
                put(c, "tasks", Some(&params.option_list("goals").join(" ")));
                put(c, "arguments", params.option("cmdOptions"));
            }
            let runner =
                settings.and_then(|s| s.option_node("myRunnerSettings")).and_then(|o| o.child("MavenRunnerSettings"));
            put(c, "vmOptions", runner.and_then(|r| r.option("vmOptions")));
        }
        "application" | "spring-boot" => {
            let spring = kind == "spring-boot";
            put(c, "mainClass", e.option(if spring { "SPRING_BOOT_MAIN_CLASS" } else { "MAIN_CLASS_NAME" }));
            if spring {
                put(c, "activeProfiles", e.option("ACTIVE_PROFILES"));
            }
            put(c, "programArgs", e.option("PROGRAM_PARAMETERS"));
            put(c, "vmOptions", e.option("VM_PARAMETERS"));
            set_cwd(c, root, e.option("WORKING_DIRECTORY"));
            c.extras.include_system_env = e.option("PASS_PARENT_ENVS") != Some("false");
        }
        "remote-jvm" => {
            put(c, "mode", Some(if e.option("SERVER_MODE") == Some("true") { "listen" } else { "attach" }));
            put(c, "host", e.option("HOST"));
            put(c, "port", e.option("PORT"));
        }
        "python-remote" => {
            put(c, "host", e.option("HOST"));
            put(c, "port", e.option("PORT"));
            let mapping = e
                .child("PathMappingSettings")
                .and_then(|s| s.option_node("pathMappings"))
                .and_then(|o| o.child("list"))
                .and_then(|l| l.child("mapping"));
            if let Some(m) = mapping {
                put(c, "localRoot", rel(m.attr("local-root")).as_deref());
                put(c, "remoteRoot", m.attr("remote-root"));
            }
        }
        "node-remote" | "go-remote" => {
            put(c, "host", e.attr("host"));
            put(c, "port", e.attr("port"));
            if let Some(m) = e.child("mapping") {
                put(c, "remoteRoot", m.attr("url"));
                put(c, "localRoot", rel(m.attr("local-file")).as_deref());
            }
        }
        "docker-dockerfile" | "docker-image" | "docker-compose" => {
            put(c, "connection", e.attr("server-name"));
            let Some(s) = e.child("deployment").and_then(|d| d.child("settings")) else { return };
            match kind {
                "docker-dockerfile" => {
                    put(c, "tag", s.option("imageTag"));
                    put(c, "containerName", s.option("containerName"));
                    put(c, "context", rel(s.option("contextFolderPath")).as_deref());
                    put(c, "dockerfile", rel(s.option("sourceFilePath")).as_deref());
                }
                "docker-image" => {
                    put(c, "image", s.option("imageTag"));
                    put(c, "containerName", s.option("containerName"));
                }
                _ => put(c, "files", rel(s.option("sourceFilePath")).as_deref()),
            }
        }
        COMPOUND => {}
        _ => {
            c.command = e.option("command").unwrap_or_default().to_owned();
            c.args = e.option_list("args");
            set_cwd(c, root, e.option("cwd"));
            for param in e.child("params").into_iter().flat_map(|p| p.elements()) {
                if let (Some(k), Some(v)) = (param.attr("name"), param.attr("value")) {
                    c.extras.params.insert(k.to_owned(), v.to_owned());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn blank(kind: &str) -> RunConfig {
        RunConfig {
            id: "user:x".into(),
            name: "x".into(),
            kind: kind.into(),
            command: String::new(),
            args: vec![],
            cwd: None,
            env: BTreeMap::new(),
            detected: false,
            is_test: false,
            debug_adapter: None,
            allow_parallel: false,
            extras: Default::default(),
        }
    }

    /// Every param in `params` survives write → read on its own (no sidecar).
    fn round_trip(kind: &str, params: &[(&str, &str)]) {
        let root = Path::new("/p");
        let mut c = blank(kind);
        c.extras.params = params.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
        let mut e = Element::new("configuration");
        write(kind, &c, &mut e, root, true);
        let mut back = blank(kind);
        read(kind, &e, root, &mut back);
        assert_eq!(back.extras.params, c.extras.params, "{kind}: {}", e.to_idea_string());
    }

    #[test]
    fn kinds_map_both_ways() {
        for (kind, ty, factory) in TABLE {
            assert_eq!(idea_type(kind), (*ty, *factory));
            assert_eq!(kind_of(ty, *factory).as_deref(), Some(*kind));
        }
        assert_eq!(idea_type("godot"), (FALLBACK, Some("godot")));
        assert_eq!(kind_of(FALLBACK, Some("godot")).as_deref(), Some("godot"));
        assert_eq!(kind_of("JUnit", Some("JUnit")), None);
        // IntelliJ's own factory names don't matter outside docker
        assert_eq!(kind_of("Application", None).as_deref(), Some("application"));
    }

    #[test]
    fn shell_round_trip() {
        round_trip("shell", &[("mode", "file"), ("path", "a.sh"), ("options", "-x"), ("interpreter", "/bin/zsh")]);
        round_trip("shell", &[("mode", "text"), ("script", "echo hi")]);
    }

    #[test]
    fn npm_round_trip() {
        round_trip("npm", &[("packageDir", "web"), ("command", "run"), ("script", "dev"), ("packageManager", "pnpm")]);
    }

    #[test]
    fn gradle_round_trip() {
        round_trip("gradle", &[("projectDir", "app"), ("tasks", "clean build"), ("arguments", "--info")]);
    }

    #[test]
    fn maven_round_trip() {
        round_trip("maven", &[("projectDir", "svc"), ("tasks", "clean install"), ("vmOptions", "-Xmx1g")]);
    }

    #[test]
    fn application_round_trip() {
        round_trip("application", &[("mainClass", "a.Main"), ("programArgs", "--x"), ("vmOptions", "-ea")]);
        round_trip("spring-boot", &[("mainClass", "a.App"), ("activeProfiles", "dev")]);
    }

    #[test]
    fn remote_round_trip() {
        round_trip("remote-jvm", &[("mode", "listen"), ("host", "h"), ("port", "5005")]);
        round_trip("python-remote", &[("host", "h"), ("port", "5678"), ("localRoot", "src"), ("remoteRoot", "/app")]);
        round_trip("node-remote", &[("host", "h"), ("port", "9229"), ("localRoot", "src"), ("remoteRoot", "/app")]);
        round_trip("go-remote", &[("host", "h"), ("port", "2345")]);
    }

    #[test]
    fn docker_round_trip() {
        round_trip("docker-dockerfile", &[("dockerfile", "Dockerfile"), ("tag", "app:dev"), ("connection", "Docker")]);
        round_trip("docker-image", &[("image", "postgres:16"), ("containerName", "db")]);
        round_trip("docker-compose", &[("files", "compose.yml")]);
    }

    #[test]
    fn fallback_round_trip() {
        round_trip("godot", &[("projectDir", "game"), ("scene", "main.tscn")]);
    }
}
