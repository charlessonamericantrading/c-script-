// Tests de integración para pushdown de `IN` (conjuntos) y búsquedas de texto
// (`contains`, `startsWith`, `endsWith`) a SQL (PLAN.md §9.20 Fase 2.1 y 2.2).

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let unique = format!(
            "linkc-pushdown-{name}-{}-{}",
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
        let _ = std::fs::remove_file(&self.0);
    }
}

fn run_link_tests(source: &str) -> (bool, String) {
    let temp = TempDir::new("run");
    let src = temp.write("app.link", source);
    let out = Command::new(env!("CARGO_BIN_EXE_linkc")).arg("test").arg(&src).output().expect("linkc test");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

const PROGRAM: &str = r#"
type Product = {
  id: Int,
  name: String,
  code: String,
  category: String,
  price: Int,
}

type NewProduct = {
  name: String,
  code: String,
  category: String,
  price: Int,
}

db {
  products: Product[],
}

service Catalog {
  rpc add(name: String, code: String, category: String, price: Int) -> Product {
    db.products.insert(NewProduct {
      name: name,
      code: code,
      category: category,
      price: price,
    })
  }

  rpc byIds(ids: Int[]) -> Product[] {
    db.products.findWhere(|p: Product| { ids.contains(p.id) })
  }

  rpc byCategory(cat: String) -> Product[] {
    db.products.findWhere(|p: Product| { p.category == cat })
  }

  rpc searchName(query: String) -> Product[] {
    db.products.findWhere(|p: Product| { p.name.contains(query) })
  }

  rpc byCodePrefix(prefix: String) -> Product[] {
    db.products.findWhere(|p: Product| { p.code.startsWith(prefix) })
  }

  rpc byCodeSuffix(suffix: String) -> Product[] {
    db.products.findWhere(|p: Product| { p.code.endsWith(suffix) })
  }

  rpc countInCategories(cats: String[]) -> Int {
    db.products.countWhere(|p: Product| { cats.contains(p.category) })
  }

  rpc removeCodes(codes: String[]) -> Int {
    db.products.deleteWhere(|p: Product| { codes.contains(p.code) })
  }
}

test "pushdown IN and string matching operations work end-to-end" {
  let p1 = Catalog.add("Gaming Laptop Pro", "ELEC-LAP-01", "Electronics", 1500);
  let p2 = Catalog.add("Office Laptop Basic", "ELEC-LAP-02", "Electronics", 700);
  let p3 = Catalog.add("Mechanical Keyboard", "ELEC-KB-03", "Electronics", 120);
  let p4 = Catalog.add("Ergonomic Chair", "FURN-CH-04", "Furniture", 350);
  let p5 = Catalog.add("Standing Desk", "FURN-DK-05", "Furniture", 500);

  // 1. IN con lista de identificadores
  let targetIds = [p1.id, p3.id, p5.id];
  let found = Catalog.byIds(targetIds);
  assert(found.length() == 3, "IN encuentra exactamente los 3 items");

  // 2. IN con lista vacía
  let emptyIds: Int[] = [];
  assert(Catalog.byIds(emptyIds).length() == 0, "IN con lista vacía devuelve 0 items");

  // 3. StringMatch .contains()
  let laptops = Catalog.searchName("Laptop");
  assert(laptops.length() == 2, "debe encontrar 2 laptops con .contains()");

  // 4. StringMatch .startsWith()
  let electronics = Catalog.byCodePrefix("ELEC-");
  assert(electronics.length() == 3, "debe encontrar 3 productos con prefijo ELEC-");

  // 5. StringMatch .endsWith()
  let chairs = Catalog.byCodeSuffix("-04");
  assert(chairs.length() == 1, "debe encontrar el producto con sufijo -04");
  assert(chairs[0].name == "Ergonomic Chair", "producto correcto");

  // 6. countWhere con IN
  let countElec = Catalog.countInCategories(["Electronics", "Furniture"]);
  assert(countElec == 5, "countWhere cuenta los 5 items de ambas categorias");

  // 7. deleteWhere con IN
  let deletedCount = Catalog.removeCodes(["ELEC-KB-03", "FURN-DK-05"]);
  assert(deletedCount == 2, "deleteWhere borra los 2 items especificados");
  assert(Catalog.byIds([p3.id, p5.id]).length() == 0, "items borrados ya no se encuentran");
}
"#;

#[test]
fn pushdown_in_and_string_methods_work_end_to_end_through_real_binary() {
    let (ok, out) = run_link_tests(PROGRAM);
    assert!(ok, "test falló: {out}");
}
