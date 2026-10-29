//! Data sources in IntelliJ's files: connection settings in `.idea/dataSources.xml`
//! (shared), the user name in `.idea/dataSources.local.xml` (per user, git-ignored).
//! Passwords stay in [`crate::secrets`], keyed by the id, which is IntelliJ's `uuid`.
//! Settings IntelliJ has no element for go to `.idea/openidea/data-sources.toml`,
//! as the difference between a source and what its XML reads back as.

use crate::source::{DataSource, DbDriver, DbProperty, DbSslMode, parse_url};
use ide_core::idea_xml::{self, Element, component, component_mut};
use ide_core::{IdeError, IdeResult, project_dir};
use serde_json::{Map, Value as Json};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

const MANAGER: &str = "DataSourceManagerImpl";
const LOCAL: &str = "dataSourceStorageLocal";
const SIDECAR_TABLE: &str = "source";
const UNSET: &str = "unset";

fn shared_file(root: &Path) -> PathBuf {
    project_dir::idea_path(root, "dataSources.xml")
}

fn local_file(root: &Path) -> PathBuf {
    project_dir::idea_path(root, "dataSources.local.xml")
}

fn sidecar_file(root: &Path) -> PathBuf {
    project_dir::path(root, "data-sources.toml")
}

fn text_of<'a>(e: &'a Element, name: &str) -> Option<String> {
    e.child(name).map(Element::text)
}

fn set_text(e: &mut Element, name: &str, v: Option<&str>) {
    match v {
        Some(v) => e.child_or_insert(name).set_text(v),
        None => e.retain_elements(|c| c.name != name),
    }
}

fn driver_of(driver_ref: &str) -> Option<(DbDriver, &'static str)> {
    match driver_ref {
        "postgresql" => Some((DbDriver::Postgres, "")),
        "mariadb" => Some((DbDriver::Mysql, "mariadb")),
        r if r.starts_with("mysql") => Some((DbDriver::Mysql, "")),
        r if r.starts_with("sqlite") => Some((DbDriver::Sqlite, "")),
        _ => None,
    }
}

/// IntelliJ's `driver-ref`, JDBC driver class and URL scheme.
fn idea_driver(s: &DataSource) -> (&'static str, &'static str, &'static str) {
    match s.driver {
        DbDriver::Postgres => ("postgresql", "org.postgresql.Driver", "postgresql"),
        DbDriver::Mysql if s.vendor == "mariadb" => ("mariadb", "org.mariadb.jdbc.Driver", "mariadb"),
        DbDriver::Mysql => ("mysql.8", "com.mysql.cj.jdbc.Driver", "mysql"),
        DbDriver::Sqlite => ("sqlite.xerial", "org.sqlite.JDBC", "sqlite"),
    }
}

fn ssl_value(m: DbSslMode) -> &'static str {
    match m {
        DbSslMode::Disable => "disable",
        DbSslMode::Prefer => "prefer",
        DbSslMode::Require => "require",
        DbSslMode::VerifyCa => "verify-ca",
        DbSslMode::VerifyFull => "verify-full",
    }
}

fn ssl_mode(v: &str) -> Option<DbSslMode> {
    [DbSslMode::Disable, DbSslMode::Prefer, DbSslMode::Require, DbSslMode::VerifyCa, DbSslMode::VerifyFull]
        .into_iter()
        .find(|m| ssl_value(*m) == v)
}

fn jdbc_url(root: &Path, s: &DataSource) -> String {
    if s.url_only {
        let url = s.url.trim();
        return if url.starts_with("jdbc:") { url.to_owned() } else { format!("jdbc:{url}") };
    }
    let (_, _, scheme) = idea_driver(s);
    if s.driver == DbDriver::Sqlite {
        let file = s.file.as_ref().map(|f| root.join(f)).unwrap_or_default();
        return format!("jdbc:sqlite:{}", idea_xml::collapse_path(root, &file.to_string_lossy()));
    }
    let host = if s.host.contains(':') { format!("[{}]", s.host) } else { s.host.clone() };
    let port = s.port.map(|p| format!(":{p}")).unwrap_or_default();
    format!("jdbc:{scheme}://{host}{port}/{}", s.database)
}

/// The data source an IntelliJ `<data-source>` pair describes; `None` for drivers OpenIDEA lacks.
fn parse(root: &Path, e: &Element, local: Option<&Element>) -> Option<DataSource> {
    let (driver, vendor) = driver_of(text_of(e, "driver-ref")?.trim())?;
    let mut s = DataSource {
        id: e.attr("uuid")?.to_owned(),
        name: e.attr("name").unwrap_or_default().to_owned(),
        driver,
        vendor: vendor.to_owned(),
        ..Default::default()
    };
    let url = text_of(e, "jdbc-url").unwrap_or_default();
    if driver == DbDriver::Sqlite {
        let raw = url.strip_prefix("jdbc:sqlite:").unwrap_or(&url);
        let abs = idea_xml::expand_path(root, raw);
        let file = Path::new(&abs).strip_prefix(root).map(Path::to_path_buf).unwrap_or_else(|_| PathBuf::from(&abs));
        s.file = (!abs.is_empty()).then(|| file.into());
    } else {
        match parse_url(&url) {
            Ok(u) => {
                s.host = u.host;
                s.port = u.port;
                s.database = u.database;
            }
            Err(_) => {
                // hmm, a URL we can't take apart still connects as is
                s.url_only = true;
                s.url = url;
            }
        }
    }
    for p in e.child("driver-properties").into_iter().flat_map(Element::elements) {
        let (Some(name), value) = (p.attr("name"), p.attr("value").unwrap_or_default()) else { continue };
        match (driver, name, ssl_mode(value)) {
            (DbDriver::Postgres, "sslmode", Some(mode)) => s.ssl = mode,
            _ => s.properties.push(DbProperty { name: name.to_owned(), value: value.to_owned() }),
        }
    }
    s.user = local.and_then(|l| text_of(l, "user-name")).unwrap_or_default();
    Some(s)
}

fn build(root: &Path, s: &DataSource, base: Option<&Element>) -> Element {
    let (driver_ref, class, _) = idea_driver(s);
    let fresh = base.is_none();
    let mut e = base.cloned().unwrap_or_else(|| {
        Element::new("data-source").with_attr("source", "LOCAL").with_attr("name", "").with_attr("uuid", s.id.as_str())
    });
    e.set_attr("name", s.name.as_str());
    set_text(&mut e, "driver-ref", Some(driver_ref));
    if fresh {
        set_text(&mut e, "synchronize", Some("true"));
    }
    set_text(&mut e, "jdbc-driver", Some(class));
    set_text(&mut e, "jdbc-url", Some(&jdbc_url(root, s)));
    let mut props: Vec<(&str, &str)> = s.properties.iter().map(|p| (p.name.as_str(), p.value.as_str())).collect();
    if s.driver == DbDriver::Postgres && s.ssl != DbSslMode::Prefer {
        props.insert(0, ("sslmode", ssl_value(s.ssl)));
    }
    if props.is_empty() {
        e.retain_elements(|c| c.name != "driver-properties");
    } else {
        let list = e.child_or_insert("driver-properties");
        list.children.clear();
        for (k, v) in props {
            list.push(Element::new("property").with_attr("name", k).with_attr("value", v));
        }
    }
    if fresh {
        set_text(&mut e, "working-dir", Some("$ProjectFileDir$"));
    }
    e
}

fn build_local(s: &DataSource, base: Option<&Element>) -> Element {
    let fresh = base.is_none();
    let mut e = base
        .cloned()
        .unwrap_or_else(|| Element::new("data-source").with_attr("name", "").with_attr("uuid", s.id.as_str()));
    e.set_attr("name", s.name.as_str());
    if fresh {
        set_text(&mut e, "secret-storage", Some("master_key"));
    }
    set_text(&mut e, "user-name", (!s.user.is_empty()).then_some(s.user.as_str()));
    if fresh {
        e.push(Element::new("schema-mapping"));
    }
    e
}

/// TOML has no null, so unset options disappear at every depth.
fn strip_nulls(v: &mut Json) {
    match v {
        Json::Object(m) => {
            m.retain(|_, v| !v.is_null());
            m.values_mut().for_each(strip_nulls);
        }
        Json::Array(a) => a.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}

fn object(s: &DataSource) -> Map<String, Json> {
    let mut v = serde_json::to_value(s).unwrap_or(Json::Null);
    strip_nulls(&mut v);
    match v {
        Json::Object(m) => m,
        _ => Map::new(),
    }
}

fn diff(original: &DataSource, lossy: &DataSource) -> Option<toml::Table> {
    let (o, l) = (object(original), object(lossy));
    let mut out: Map<String, Json> =
        o.iter().filter(|(k, v)| l.get(*k) != Some(v)).map(|(k, v)| (k.clone(), v.clone())).collect();
    let unset: Vec<Json> = l.keys().filter(|k| !o.contains_key(*k)).map(|k| Json::String(k.clone())).collect();
    if !unset.is_empty() {
        out.insert(UNSET.into(), Json::Array(unset));
    }
    if out.is_empty() {
        return None;
    }
    match toml::Value::try_from(Json::Object(out)) {
        Ok(toml::Value::Table(t)) => Some(t),
        _ => None,
    }
}

fn apply(base: DataSource, entry: &toml::Table) -> DataSource {
    let Ok(Json::Object(d)) = serde_json::to_value(entry) else { return base };
    let mut m = object(&base);
    for (k, v) in d {
        if k == UNSET {
            for key in v.as_array().into_iter().flatten().filter_map(Json::as_str) {
                m.remove(key);
            }
        } else {
            m.insert(k, v);
        }
    }
    serde_json::from_value(Json::Object(m)).unwrap_or_else(|e| {
        tracing::warn!("ignoring a stale data source sidecar entry for {}: {e}", base.name);
        base
    })
}

type Sidecar = BTreeMap<String, toml::Table>;

fn read_sidecar(root: &Path) -> Sidecar {
    let Ok(text) = std::fs::read_to_string(sidecar_file(root)) else { return Sidecar::new() };
    let Ok(mut doc) = toml::from_str::<toml::Table>(&text) else { return Sidecar::new() };
    match doc.remove(SIDECAR_TABLE) {
        Some(toml::Value::Table(t)) => t
            .into_iter()
            .filter_map(|(k, v)| match v {
                toml::Value::Table(e) => Some((k, e)),
                _ => None,
            })
            .collect(),
        _ => Sidecar::new(),
    }
}

fn write_sidecar(root: &Path, entries: &Sidecar) -> IdeResult<()> {
    let file = sidecar_file(root);
    if entries.is_empty() {
        return match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        };
    }
    let text =
        toml::to_string(&BTreeMap::from([(SIDECAR_TABLE, entries)])).map_err(|e| IdeError::internal(e.to_string()))?;
    if std::fs::read_to_string(&file).ok().as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    idea_xml::write_atomic(&file, &text)
}

fn sources_of(project: Option<&Element>, name: &str) -> Vec<Element> {
    project
        .and_then(|p| component(p, name))
        .map(|c| c.elements().filter(|e| e.name == "data-source").cloned().collect())
        .unwrap_or_default()
}

fn supported(e: &Element) -> bool {
    text_of(e, "driver-ref").is_some_and(|r| driver_of(r.trim()).is_some())
}

pub(crate) fn load(root: &Path) -> IdeResult<Vec<DataSource>> {
    let shared = idea_xml::read_project_file(&shared_file(root))?;
    let local = idea_xml::read_project_file(&local_file(root))?;
    let locals: HashMap<String, Element> =
        sources_of(local.as_ref(), LOCAL).into_iter().filter_map(|e| Some((e.attr("uuid")?.to_owned(), e))).collect();
    let side = read_sidecar(root);
    Ok(sources_of(shared.as_ref(), MANAGER)
        .iter()
        .filter_map(|e| {
            let s = parse(root, e, e.attr("uuid").and_then(|id| locals.get(id)))?;
            Some(match side.get(&s.id) {
                Some(entry) => apply(s, entry),
                None => s,
            })
        })
        .collect())
}

/// Puts `ours` into the component `name` of `file` instead of the sources OpenIDEA owns
/// (supported drivers, or ids in `ids`), keeping the rest.
fn replace_in(
    file: &Path,
    name: &str,
    ids: &[&str],
    own: impl Fn(&Element) -> bool,
    ours: Vec<Element>,
    attrs: &[(&str, &str)],
) -> IdeResult<()> {
    idea_xml::update_project_file(file, |p| {
        let c = component_mut(p, name);
        c.retain_elements(|e| {
            !(e.name == "data-source" && (own(e) || e.attr("uuid").is_some_and(|id| ids.contains(&id))))
        });
        let empty = ours.is_empty();
        for e in ours {
            c.push(e);
        }
        if !empty {
            for (k, v) in attrs {
                if c.attr(k).is_none() {
                    c.set_attr(k, *v);
                }
            }
        }
        if c.children.is_empty() {
            p.retain_elements(|e| !(e.name == "component" && e.attr("name") == Some(name)));
        }
    })
}

pub(crate) fn save(root: &Path, sources: &[DataSource]) -> IdeResult<()> {
    let shared = idea_xml::read_project_file(&shared_file(root))?;
    let local = idea_xml::read_project_file(&local_file(root))?;
    let by_id = |els: Vec<Element>| -> HashMap<String, Element> {
        els.into_iter().filter_map(|e| Some((e.attr("uuid")?.to_owned(), e))).collect()
    };
    let (old_shared, old_local) =
        (by_id(sources_of(shared.as_ref(), MANAGER)), by_id(sources_of(local.as_ref(), LOCAL)));
    let mut side = Sidecar::new();
    let (mut ours, mut ours_local) = (Vec::new(), Vec::new());
    for s in sources {
        let e = build(root, s, old_shared.get(&s.id));
        let l = build_local(s, old_local.get(&s.id));
        let lossy = parse(root, &e, Some(&l)).expect("built from a supported driver");
        if let Some(d) = diff(s, &lossy) {
            side.insert(s.id.clone(), d);
        }
        ours.push(e);
        ours_local.push(l);
    }
    let ids: Vec<&str> = sources.iter().map(|s| s.id.as_str()).collect();
    // a local entry is ours when its shared twin is (the local file has no driver)
    let owned_local: Vec<String> = old_shared.iter().filter(|(_, e)| supported(e)).map(|(id, _)| id.clone()).collect();
    replace_in(&shared_file(root), MANAGER, &ids, supported, ours, &[("format", "xml"), ("multifile-model", "true")])?;
    replace_in(
        &local_file(root),
        LOCAL,
        &ids,
        |e| e.attr("uuid").is_some_and(|id| owned_local.iter().any(|o| o == id)),
        ours_local,
        &[],
    )?;
    write_sidecar(root, &side)?;
    if !sources.is_empty() {
        project_dir::ensure_gitignore(root)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::source::*;
    use std::path::Path;

    fn write(root: &Path, file: &str, text: &str) {
        let p = root.join(".idea").join(file);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    const SOURCES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<project version="4">
  <component name="DataSourceManagerImpl" format="xml" multifile-model="true">
    <data-source source="LOCAL" name="shop" uuid="0f1e2d3c-aaaa-bbbb-cccc-000000000001">
      <driver-ref>postgresql</driver-ref>
      <synchronize>true</synchronize>
      <jdbc-driver>org.postgresql.Driver</jdbc-driver>
      <jdbc-url>jdbc:postgresql://db.local:5433/shop</jdbc-url>
      <driver-properties>
        <property name="sslmode" value="require" />
        <property name="application_name" value="ide" />
      </driver-properties>
      <working-dir>$ProjectFileDir$</working-dir>
    </data-source>
    <data-source source="LOCAL" name="legacy" uuid="0f1e2d3c-aaaa-bbbb-cccc-000000000002">
      <driver-ref>oracle</driver-ref>
      <jdbc-driver>oracle.jdbc.OracleDriver</jdbc-driver>
      <jdbc-url>jdbc:oracle:thin:@localhost:1521:XE</jdbc-url>
    </data-source>
  </component>
</project>"#;

    const LOCAL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<project version="4">
  <component name="dataSourceStorageLocal" created-in="IU-252.1">
    <data-source name="shop" uuid="0f1e2d3c-aaaa-bbbb-cccc-000000000001">
      <database-info product="PostgreSQL" version="16.1" />
      <secret-storage>master_key</secret-storage>
      <user-name>ann</user-name>
      <schema-mapping />
    </data-source>
  </component>
</project>"#;

    #[test]
    fn reads_intellij_postgres_source() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "dataSources.xml", SOURCES);
        write(tmp.path(), "dataSources.local.xml", LOCAL);
        let s = load(tmp.path()).unwrap();
        assert_eq!(s.len(), 1, "oracle isn't supported, so it isn't listed");
        let s = &s[0];
        assert_eq!((s.id.as_str(), s.name.as_str()), ("0f1e2d3c-aaaa-bbbb-cccc-000000000001", "shop"));
        assert_eq!(s.driver, DbDriver::Postgres);
        assert_eq!((s.host.as_str(), s.port, s.database.as_str()), ("db.local", Some(5433), "shop"));
        assert_eq!(s.user, "ann");
        assert_eq!(s.ssl, DbSslMode::Require);
        assert_eq!(s.properties, vec![DbProperty { name: "application_name".into(), value: "ide".into() }]);
    }

    #[test]
    fn id_round_trips_as_uuid() {
        let tmp = tempfile::tempdir().unwrap();
        let s = DataSource { id: "ab12".into(), name: "x".into(), driver: DbDriver::Postgres, ..Default::default() };
        save(tmp.path(), std::slice::from_ref(&s)).unwrap();
        let xml = std::fs::read_to_string(tmp.path().join(".idea/dataSources.xml")).unwrap();
        assert!(xml.contains(r#"<data-source source="LOCAL" name="x" uuid="ab12">"#), "{xml}");
        assert_eq!(load(tmp.path()).unwrap()[0].id, "ab12");
    }

    #[test]
    fn save_keeps_unsupported_driver_sources() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "dataSources.xml", SOURCES);
        write(tmp.path(), "dataSources.local.xml", LOCAL);
        let mut s = load(tmp.path()).unwrap();
        s[0].database = "shop2".into();
        save(tmp.path(), &s).unwrap();
        let xml = std::fs::read_to_string(tmp.path().join(".idea/dataSources.xml")).unwrap();
        assert!(xml.contains("<jdbc-url>jdbc:oracle:thin:@localhost:1521:XE</jdbc-url>"), "{xml}");
        assert!(xml.contains("<jdbc-url>jdbc:postgresql://db.local:5433/shop2</jdbc-url>"), "{xml}");
        let local = std::fs::read_to_string(tmp.path().join(".idea/dataSources.local.xml")).unwrap();
        assert!(local.contains(r#"<database-info product="PostgreSQL" version="16.1" />"#), "{local}");
        assert!(local.contains("created-in=\"IU-252.1\""), "{local}");
        assert_eq!(load(tmp.path()).unwrap(), s);

        save(tmp.path(), &[]).unwrap();
        let xml = std::fs::read_to_string(tmp.path().join(".idea/dataSources.xml")).unwrap();
        assert!(xml.contains("oracle") && !xml.contains("postgresql"), "{xml}");
    }

    #[test]
    fn sqlite_file_uses_project_macro() {
        let tmp = tempfile::tempdir().unwrap();
        let s = DataSource {
            id: "s".into(),
            name: "dev.db".into(),
            driver: DbDriver::Sqlite,
            file: Some(std::path::PathBuf::from("data/dev.db").into()),
            ..Default::default()
        };
        save(tmp.path(), std::slice::from_ref(&s)).unwrap();
        let xml = std::fs::read_to_string(tmp.path().join(".idea/dataSources.xml")).unwrap();
        assert!(xml.contains("<jdbc-url>jdbc:sqlite:$PROJECT_DIR$/data/dev.db</jdbc-url>"), "{xml}");
        assert!(xml.contains("<driver-ref>sqlite.xerial</driver-ref>"), "{xml}");
        assert_eq!(load(tmp.path()).unwrap(), vec![s]);
    }
}
