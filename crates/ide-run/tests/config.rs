use ide_run::{BeforeLaunchTask, RunConfig, load_templates, load_user_configs, save_templates, save_user_configs};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn config(id: &str, name: &str, kind: &str) -> RunConfig {
    RunConfig {
        id: id.into(),
        name: name.into(),
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

fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
}

fn sample() -> Vec<RunConfig> {
    let mut server = config("user:server", "Server \"prod\"", "shell");
    server.command = "cargo".into();
    server.args = vec!["run".into(), "--release".into()];
    server.cwd = Some(PathBuf::from("server").into());
    server.env = BTreeMap::from([("RUST_LOG".into(), "debug".into()), ("A B".into(), "x=y".into())]);
    server.debug_adapter = Some("codelldb".into());
    server.allow_parallel = true;
    let mut tests = config("user:tests", "Tests", "pytest");
    tests.command = "pytest -q tests/".into();
    tests.is_test = true;
    vec![server, tests]
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn missing_files_are_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(load_user_configs(dir.path()).unwrap().is_empty());
    // Saving nothing creates nothing.
    save_user_configs(dir.path(), &[]).unwrap();
    assert!(!dir.path().join(".idea").exists());
}

#[test]
fn round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let configs = sample();
    save_user_configs(dir.path(), &configs).unwrap();
    assert!(dir.path().join(".idea/runConfigurations/Server__prod_.xml").is_file());
    assert!(dir.path().join(".idea/runConfigurations/Tests.xml").is_file());
    assert_eq!(load_user_configs(dir.path()).unwrap(), configs);

    // Saving a detected config persists it as a user config.
    let mut detected = configs[0].clone();
    detected.detected = true;
    save_user_configs(dir.path(), &[detected]).unwrap();
    let loaded = load_user_configs(dir.path()).unwrap();
    assert_eq!(loaded.len(), 1);
    assert!(!loaded[0].detected);
    assert!(!dir.path().join(".idea/runConfigurations/Tests.xml").exists());
}

#[test]
fn shared_config_writes_run_configurations_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut dev = config("user:dev-server", "Dev server", "shell");
    dev.extras.params = params(&[("mode", "file"), ("path", "scripts/dev.sh")]);
    dev.command = "bash".into();
    dev.args = vec!["scripts/dev.sh".into()];
    save_user_configs(dir.path(), std::slice::from_ref(&dev)).unwrap();
    assert_eq!(
        read(dir.path().join(".idea/runConfigurations/Dev_server.xml")),
        r#"<component name="ProjectRunConfigurationManager">
  <configuration default="false" name="Dev server" type="ShConfigurationType">
    <option name="SCRIPT_TEXT" value="" />
    <option name="INDEPENDENT_SCRIPT_PATH" value="true" />
    <option name="SCRIPT_PATH" value="$PROJECT_DIR$/scripts/dev.sh" />
    <option name="SCRIPT_OPTIONS" value="" />
    <option name="INDEPENDENT_SCRIPT_WORKING_DIRECTORY" value="true" />
    <option name="SCRIPT_WORKING_DIRECTORY" value="$PROJECT_DIR$" />
    <option name="INDEPENDENT_INTERPRETER_PATH" value="true" />
    <option name="INTERPRETER_PATH" value="" />
    <option name="INTERPRETER_OPTIONS" value="" />
    <option name="EXECUTE_IN_TERMINAL" value="false" />
    <option name="EXECUTE_SCRIPT_FILE" value="true" />
    <envs />
    <method v="2" />
  </configuration>
</component>"#
    );
    assert_eq!(load_user_configs(dir.path()).unwrap(), vec![dev]);
}

#[test]
fn intellij_written_config_loads_with_params() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(".idea/runConfigurations/Build.xml");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        &file,
        "<component name=\"ProjectRunConfigurationManager\">\n\
  <configuration default=\"false\" name=\"Build\" type=\"GradleRunConfiguration\" factoryName=\"Gradle\" folderName=\"Tools\">\n\
    <ExternalSystemSettings>\n\
      <option name=\"externalProjectPath\" value=\"$PROJECT_DIR$/app\" />\n\
      <option name=\"externalSystemIdString\" value=\"GRADLE\" />\n\
      <option name=\"scriptParameters\" value=\"--info\" />\n\
      <option name=\"taskNames\">\n\
        <list>\n\
          <option value=\"clean\" />\n\
          <option value=\"build\" />\n\
        </list>\n\
      </option>\n\
    </ExternalSystemSettings>\n\
    <method v=\"2\" />\n\
  </configuration>\n\
</component>",
    )
    .unwrap();
    let c = load_user_configs(dir.path()).unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].id, "user:build");
    assert_eq!(c[0].name, "Build");
    assert_eq!(c[0].kind, "gradle");
    assert_eq!(c[0].extras.folder.as_deref(), Some("Tools"));
    assert_eq!(c[0].extras.params, params(&[("projectDir", "app"), ("tasks", "clean build"), ("arguments", "--info")]));
    assert!(!c[0].extras.local);
}

#[test]
fn local_config_and_template_go_to_workspace_xml() {
    let dir = tempfile::tempdir().unwrap();
    let mut npm = config("user:dev", "dev", "npm");
    npm.extras.local = true;
    npm.extras.params = params(&[("command", "run"), ("script", "dev")]);
    save_user_configs(dir.path(), std::slice::from_ref(&npm)).unwrap();
    let mut template = config("template:gradle", "", "gradle");
    template.extras.params = params(&[("tasks", "build")]);
    save_templates(dir.path(), std::slice::from_ref(&template)).unwrap();

    let ws = read(dir.path().join(".idea/workspace.xml"));
    assert!(ws.contains("<component name=\"RunManager\">"), "{ws}");
    assert!(ws.contains("name=\"dev\" type=\"js.build_tools.npm\""), "{ws}");
    assert!(
        ws.contains("<configuration default=\"true\" type=\"GradleRunConfiguration\" factoryName=\"Gradle\">"),
        "{ws}"
    );
    assert!(!dir.path().join(".idea/runConfigurations").exists());
    assert_eq!(load_user_configs(dir.path()).unwrap(), vec![npm]);
    assert_eq!(load_templates(dir.path()).unwrap(), vec![template]);
    let ignore = read(dir.path().join(".idea/.gitignore"));
    assert!(ignore.lines().any(|l| l == "/workspace.xml"), "{ignore}");
}

#[test]
fn save_keeps_unknown_type_configurations() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join(".idea/workspace.xml");
    fs::create_dir_all(ws.parent().unwrap()).unwrap();
    let junit = "<configuration name=\"AllTests\" type=\"JUnit\" factoryName=\"JUnit\">\n      <option name=\"PACKAGE_NAME\" value=\"a\" />\n      <method v=\"2\">\n        <option name=\"Make\" enabled=\"true\" />\n      </method>\n    </configuration>";
    fs::write(
        &ws,
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<project version=\"4\">\n  <component name=\"RunManager\" selected=\"JUnit.AllTests\">\n    {junit}\n    <list>\n      <item itemvalue=\"JUnit.AllTests\" />\n    </list>\n  </component>\n  <component name=\"ToolWindowManager\" />\n</project>"
        ),
    )
    .unwrap();
    let mut local = sample().remove(1);
    local.extras.local = true;
    save_user_configs(dir.path(), std::slice::from_ref(&local)).unwrap();
    let text = read(&ws);
    assert!(text.contains(junit), "{text}");
    assert!(text.contains("selected=\"JUnit.AllTests\""), "{text}");
    assert!(text.contains("<item itemvalue=\"JUnit.AllTests\" />"), "{text}");
    assert!(text.contains("<component name=\"ToolWindowManager\" />"), "{text}");
    assert_eq!(load_user_configs(dir.path()).unwrap(), vec![local]);

    save_user_configs(dir.path(), &[]).unwrap();
    assert!(read(&ws).contains(junit));
}

#[test]
fn rename_rewrites_references() {
    let dir = tempfile::tempdir().unwrap();
    let mut api = config("user:api", "api", "application");
    api.extras.params = params(&[("mainClass", "a.Api")]);
    let mut all = config("user:all", "All", "compound");
    all.extras.compound = vec!["user:api".into()];
    save_user_configs(dir.path(), &[api.clone(), all.clone()]).unwrap();

    api.name = "API server".into();
    save_user_configs(dir.path(), &[api.clone(), all.clone()]).unwrap();
    assert!(!dir.path().join(".idea/runConfigurations/api.xml").exists());
    let compound = read(dir.path().join(".idea/runConfigurations/All.xml"));
    assert!(compound.contains("<toRun name=\"API server\" type=\"Application\" />"), "{compound}");
    assert_eq!(load_user_configs(dir.path()).unwrap(), vec![api, all]);
}

#[test]
fn reads_dot_run_folder_and_writes_back_there() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(".run/Main.run.xml");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        &file,
        "<component name=\"ProjectRunConfigurationManager\">\n  <configuration default=\"false\" name=\"Main\" type=\"Application\" factoryName=\"Application\">\n    <option name=\"MAIN_CLASS_NAME\" value=\"a.Main\" />\n    <module name=\"app\" />\n    <method v=\"2\" />\n  </configuration>\n</component>",
    )
    .unwrap();
    let mut c = load_user_configs(dir.path()).unwrap();
    assert_eq!(c[0].extras.params, params(&[("mainClass", "a.Main")]));
    c[0].extras.params.insert("programArgs".into(), "--port 80".into());
    save_user_configs(dir.path(), &c).unwrap();
    let text = read(&file);
    assert!(text.contains("<option name=\"PROGRAM_PARAMETERS\" value=\"--port 80\" />"), "{text}");
    assert!(text.contains("<module name=\"app\" />"), "{text}");
    assert!(!dir.path().join(".idea/runConfigurations").exists());
    assert_eq!(load_user_configs(dir.path()).unwrap(), c);
}

#[test]
fn sidecar_keeps_fields_intellij_lacks() {
    let dir = tempfile::tempdir().unwrap();
    let mut shared = sample().remove(0);
    shared.extras.folder = Some("Servers".into());
    shared.extras.params = params(&[("mode", "command")]);
    shared.extras.include_system_env = false;
    shared.extras.env_files = vec![".env".into(), "local.env".into()];
    shared.extras.stdin_file = Some("in.txt".into());
    shared.extras.output_file = Some("out.log".into());
    shared.extras.before_launch = vec![
        BeforeLaunchTask { kind: "build".into(), target: String::new() },
        BeforeLaunchTask { kind: "run".into(), target: "user:tests".into() },
        BeforeLaunchTask { kind: "command".into(), target: "make gen".into() },
    ];
    shared.extras.show_on_stderr = true;
    shared.extras.open_tool_window = Some(false);
    shared.extras.focus_tool_window = true;
    shared.extras.emulate_terminal = true;
    shared.extras.stop_command = Some("docker stop server".into());
    let mut local = sample().remove(1);
    local.extras.local = true;
    let mut compound = config("user:all", "All", "compound");
    compound.extras.compound = vec!["user:server".into(), "user:tests".into()];
    let configs = vec![shared, compound, local];
    save_user_configs(dir.path(), &configs).unwrap();
    assert_eq!(load_user_configs(dir.path()).unwrap(), configs);

    let sidecar = read(dir.path().join(".idea/openidea/run.toml"));
    assert!(sidecar.contains("stopCommand = \"docker stop server\""), "{sidecar}");
    assert!(sidecar.contains("stdinFile = \"in.txt\""), "{sidecar}");
    let xml = read(dir.path().join(".idea/runConfigurations/Server__prod_.xml"));
    assert!(xml.contains("<output_file path=\"$PROJECT_DIR$/out.log\" is_save=\"true\" />"), "{xml}");
    assert!(
        xml.contains("<option name=\"RunConfigurationTask\" enabled=\"true\" run_configuration_name=\"Tests\" run_configuration_type=\"OpenIdeaRunConfiguration\" />"),
        "{xml}"
    );
    let ws = read(dir.path().join(".idea/workspace.xml"));
    assert!(ws.contains("name=\"Tests\""), "{ws}");

    // Making it a project file moves it out of the workspace.
    let mut moved = configs.clone();
    moved[2].extras.local = false;
    save_user_configs(dir.path(), &moved).unwrap();
    assert_eq!(load_user_configs(dir.path()).unwrap(), moved);
    assert!(!read(dir.path().join(".idea/workspace.xml")).contains("name=\"Tests\""));
}

#[test]
fn templates_do_not_disturb_configurations() {
    let dir = tempfile::tempdir().unwrap();
    let configs = sample();
    save_user_configs(dir.path(), &configs).unwrap();
    let mut template = sample().remove(1);
    template.id = "template:npm".into();
    template.kind = "npm".into();
    save_templates(dir.path(), std::slice::from_ref(&template)).unwrap();
    assert_eq!(load_templates(dir.path()).unwrap(), vec![template]);
    assert_eq!(load_user_configs(dir.path()).unwrap(), configs);
}

#[test]
fn jvm_options_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let vm = "-Ddemo.flag=on --add-opens java.base/java.lang=ALL-UNNAMED '-Dname=a b'";
    let mut app = config("user:demo", "demo", "spring-boot");
    app.command = "java".into();
    app.args = ["-Ddemo.flag=on", "-cp", "${classpath}", "a.DemoApplication"].map(String::from).to_vec();
    app.extras.params = params(&[
        ("mainClass", "a.DemoApplication"),
        ("vmOptions", vm),
        ("activeProfiles", "dev"),
        ("classpath", "${classpath}"),
    ]);
    save_user_configs(dir.path(), std::slice::from_ref(&app)).unwrap();
    let text = read(dir.path().join(".idea/runConfigurations/demo.xml"));
    assert!(text.contains("type=\"SpringBootApplicationConfigurationType\" factoryName=\"Spring Boot\""), "{text}");
    assert!(text.contains("<option name=\"SPRING_BOOT_MAIN_CLASS\" value=\"a.DemoApplication\" />"), "{text}");
    assert!(text.contains("<option name=\"ACTIVE_PROFILES\" value=\"dev\" />"), "{text}");
    assert!(text.contains(&format!("<option name=\"VM_PARAMETERS\" value=\"{vm}\" />")), "{text}");
    assert_eq!(load_user_configs(dir.path()).unwrap(), vec![app]);
}

#[test]
fn invalid_file_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(".idea/runConfigurations/Broken.xml");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "<component name=\"ProjectRunConfigurationManager\"><configuration").unwrap();
    let err = load_user_configs(dir.path()).unwrap_err();
    assert_eq!(err.kind, ide_core::error::ErrorKind::InvalidInput);
    assert!(err.message.contains("Broken.xml"), "{}", err.message);
}

#[test]
fn legacy_files_still_read() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(".openidea/run.toml");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        &file,
        "[[configurations]]\nname = \"Dev Server\"\ncommand = \"npm run dev\"\n\n[[templates]]\nkind = \"npm\"\n",
    )
    .unwrap();
    let c = ide_run::legacy_user_configs(dir.path()).unwrap();
    assert_eq!(c[0].id, "user:dev-server");
    assert_eq!(c[0].kind, "shell");
    assert_eq!(ide_run::legacy_templates(dir.path()).unwrap()[0].kind, "npm");
}
