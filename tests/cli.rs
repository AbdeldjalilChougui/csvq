use assert_cmd::Command;
use predicates::str::contains;
use std::path::PathBuf;

fn data(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name).to_string_lossy().into_owned()
}

fn csvq() -> Command {
    Command::cargo_bin("csvq").unwrap()
}

fn stdout(args: &[&str]) -> String {
    let out = csvq().args(args).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn basic_commands() {
    let p = data("people.csv");
    assert_eq!(stdout(&["count", &p]), "5\n");
    assert_eq!(stdout(&["headers", &p]), "1\tid\n2\tname\n3\tage\n4\tcity\n");
    assert_eq!(stdout(&["select", "city,name", &p]).lines().nth(1), Some("Oran,Amina"));
    assert_eq!(stdout(&["search", "-s", "city", "-i", "algiers", &p]).lines().count(), 3);
    assert_eq!(stdout(&["slice", "-i", "3", &p]), "id,name,age,city\n4,Yacine,41,Constantine\n");
    assert_eq!(stdout(&["slice", "-s", "1", "-l", "1", &p]).lines().nth(1), Some("2,Karim,28,Algiers"));
    assert_eq!(stdout(&["sort", "-s", "age", "-N", &p]).lines().nth(1), Some("2,Karim,28,Algiers"));
    assert!(stdout(&["frequency", "-s", "city", &p]).contains("city,Algiers,2"));
    let stats = stdout(&["stats", "--cardinality", &p]);
    assert!(stats.contains("age,Integer,5,1,28,41,2,2,32.75,"), "{stats}");
}

#[test]
fn stdin_and_delimiter() {
    csvq()
        .args(["-d", ";", "select", "2"])
        .write_stdin("a;b\n1;2\n")
        .assert()
        .success()
        .stdout("b\n2\n");
    csvq().args(["-d", "tab", "count"]).write_stdin("a\tb\n1\t2\n3\t4\n").assert().success().stdout("2\n");
}

#[test]
fn joins() {
    let out = stdout(&["join", "city", &data("people.csv"), "city", &data("cities.csv")]);
    assert_eq!(out.lines().count(), 5);
    assert!(out.contains("1,Amina,34,Oran,Oran,31,1560000"));
    let out = stdout(&["join", "--left", "city", &data("people.csv"), "city", &data("cities.csv")]);
    assert!(out.contains("4,Yacine,41,Constantine,,,"));
}

#[test]
fn errors_are_reported() {
    csvq().args(["select", "nope", &data("people.csv")]).assert().failure().stderr(contains("no column named 'nope'"));
    csvq().args(["count", "/no/such/file.csv"]).assert().failure().stderr(contains("opening"));
    csvq().args(["sql", "SELECT 1"]).assert().failure().stderr(contains("no .csv"));
}

#[test]
fn parquet_roundtrip_and_sql() {
    let dir = tempfile::tempdir().unwrap();
    let pq = dir.path().join("people.parquet").to_string_lossy().into_owned();
    let back = dir.path().join("back.csv").to_string_lossy().into_owned();
    csvq().args(["convert", &data("people.csv"), &pq]).assert().success().stderr(contains("wrote 5 rows"));
    csvq().args(["convert", &pq, &back]).assert().success();
    let roundtrip = std::fs::read_to_string(&back).unwrap();
    assert_eq!(roundtrip, std::fs::read_to_string(data("people.csv")).unwrap());

    let q = format!("SELECT city, count(*) AS n, avg(age) AS avg_age FROM {} GROUP BY city ORDER BY n DESC, city", data("people.csv"));
    let out = stdout(&["sql", &q]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "city,n,avg_age");
    assert_eq!(lines[1], "Algiers,2,28.0");
    assert_eq!(lines[2], "Oran,2,34.0");

    // SQL over Parquet joined with CSV, using aliases.
    let q = format!(
        "SELECT p.name, c.wilaya_code FROM '{pq}' p JOIN {} c ON p.city = c.city WHERE p.age > 30 ORDER BY p.name",
        data("cities.csv")
    );
    assert_eq!(stdout(&["sql", &q]), "name,wilaya_code\nAmina,31\n");
}
