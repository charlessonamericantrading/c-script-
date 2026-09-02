// `staticRoutes(baseUrl)`, `hreflangLinks(alternates)` y `routes.json`
// (GRAMMAR.md §3.222, PLAN.md §9.18 Eje D ítems 1 y 2), contra el binario
// real: `linkc test` corre los bloques `test` del programa (el checker
// REAL tipa los dos builtins nuevos -- no el harness que lo saltea) y
// `linkc build` escribe el `routes.json` que se lee y se compara.

use std::path::PathBuf;
use std::process::Command;

const PROGRAM: &str = r#"
type Alt = { lang: String, href: String }
type Loc = { loc: String }
enum Role { Admin }

service Pages {
  @route("/home")
  @content_type("text/html; charset=utf-8")
  rpc home() -> String { "<h1>home</h1>" }

  @route("/about")
  @content_type("text/html; charset=utf-8")
  @cache_control("public, max-age=600")
  rpc about() -> String { "<h1>about</h1>" }

  @route("/blog/:slug")
  rpc post(slug: String) -> String { slug }

  @route("/files/:path*")
  rpc file(path: String) -> String { path }

  @authenticated
  @route("/me")
  rpc me() -> String { "me" }

  @requires(Role.Admin)
  @route("/admin")
  rpc admin() -> String { "admin" }

  rpc sitemap() -> String { sitemapXml(staticRoutes("https://example.com/")) }
  rpc locs() -> String { staticRoutes("https://example.com").map(|r: Loc| { r.loc }).join(",") }
  rpc head() -> String {
    hreflangLinks([Alt { lang: "es", href: "https://example.com/es" }, Alt { lang: "x-default", href: "https://example.com/" }])
  }
  rpc bad() -> String { hreflangLinks([Alt { lang: "es\"><script>", href: "https://example.com/?a=1&b=2" }]) }
}

test "staticRoutes lists exactly the public static routes, in declaration order, with the base URL joined once" {
  assert(Pages.locs() == "https://example.com/home,https://example.com/about", Pages.locs());
}

test "sitemapXml accepts staticRoutes() directly and only public static routes appear" {
  let s = Pages.sitemap();
  assert(s.contains("<loc>https://example.com/about</loc>"), "about");
  assert(s.contains("<loc>https://example.com/home</loc>"), "home");
  assert(!s.contains("blog"), "a route with a :param is not a single URL");
  assert(!s.contains("files"), "a catch-all route is not a single URL");
  assert(!s.contains("/me"), "@authenticated is excluded");
  assert(!s.contains("admin"), "@requires is excluded");
}

test "hreflangLinks emits one link element per alternate and escapes attribute values" {
  let h = Pages.head();
  assert(h == "<link rel=\"alternate\" hreflang=\"es\" href=\"https://example.com/es\">\n<link rel=\"alternate\" hreflang=\"x-default\" href=\"https://example.com/\">", h);
  let b = Pages.bad();
  assert(!b.contains("<script>"), "escaped: " + b);
  assert(b.contains("&amp;b=2"), "ampersand escaped: " + b);
}
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-routes-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&path).expect("crear tempdir");
        Self(path)
    }

    fn write(&self, name: &str, content: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, content).expect("escribir archivo");
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_three_behavior_tests_pass_through_the_real_checker_and_interpreter() {
    let temp = TempDir::new("test");
    let src = temp.write("app.link", PROGRAM);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("test").arg(&src).output().expect("linkc test");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "linkc test falló:\n{stdout}\n{stderr}");
    assert!(stdout.contains("3 passed") || stdout.contains("3 tests"), "{stdout}");
}

#[test]
fn linkc_build_writes_routes_json_with_one_entry_per_route_and_the_same_static_public_criterion() {
    let temp = TempDir::new("build");
    let src = temp.write("app.link", PROGRAM);
    let outdir = temp.0.join("gen");
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("build").arg(&src).arg(&outdir).output().expect("linkc build");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let text = std::fs::read_to_string(outdir.join("routes.json")).expect("routes.json existe");
    let routes: Vec<serde_json::Value> = serde_json::from_str(&text).expect("JSON válido");
    let paths: Vec<&str> = routes.iter().map(|r| r["path"].as_str().unwrap()).collect();
    assert_eq!(paths, vec!["/home", "/about", "/blog/:slug", "/files/:path*", "/me", "/admin"], "orden de declaración, todas las @route");

    let by_path = |p: &str| routes.iter().find(|r| r["path"] == p).unwrap();
    assert_eq!(by_path("/about")["in_sitemap"], true);
    assert_eq!(by_path("/about")["cache_control"], "public, max-age=600");
    assert_eq!(by_path("/about")["content_type"], "text/html; charset=utf-8");
    assert_eq!(by_path("/about")["service"], "Pages");
    assert_eq!(by_path("/about")["rpc"], "about");
    assert_eq!(by_path("/blog/:slug")["static"], false);
    assert_eq!(by_path("/blog/:slug")["in_sitemap"], false);
    assert_eq!(by_path("/me")["public"], false);
    assert_eq!(by_path("/me")["static"], true);
    assert_eq!(by_path("/me")["in_sitemap"], false);
    assert_eq!(by_path("/blog/:slug")["cache_control"], serde_json::Value::Null);

    let sitemap_count = routes.iter().filter(|r| r["in_sitemap"] == true).count();
    assert_eq!(sitemap_count, 2, "exactamente las mismas dos que staticRoutes() devuelve en runtime");
}
