//! GRAMMAR.md §3.210, PLAN.md §9.16 ítem 6. Códigos de error estables al
//! estilo `rustc`/`E0308` (acá `L0001`, "L" de Link) -- inspirado
//! directamente en el `E0603` real que apareció escribiendo GRAMMAR.md
//! §3.204/§3.205 en esta misma sesión: se resolvió rápido precisamente
//! porque `rustc` nombra el error con un código estable y documentado,
//! algo que `.link` no tenía.
//!
//! NO todo error tiene código -- mismo criterio pragmático que `rustc` (la
//! mayoría de sus diagnósticos tampoco lo tienen, solo un subconjunto
//! curado con explicación propia). Asignar un código a los ~113 sitios
//! `err(...)` de `checker.rs` de una sola vez sería trabajo especulativo
//! sin evidencia de qué errores realmente lo necesitan -- se arranca con
//! los que ya tienen su propia sección de GRAMMAR.md explicándolos en
//! detalle (§3.4/§3.9/§3.10/§3.206/§3.209), y se suman más códigos cuando
//! aparezca evidencia real de que otro mensaje los necesita, nunca por
//! anticipado.
//!
//! Numeración SECUENCIAL, no por categoría (`L0001`, `L0002`, ...) -- la
//! alternativa (`L-TYPE-001`, `L-PARSE-001`, etc.) es más trabajo de
//! diseño sin beneficio real: lo que importa es que el código sea
//! ESTABLE (nunca se reasigna ni se reusa un número ya dado, aunque el
//! error que describía deje de existir) y que `linkc explain <código>` dé
//! la explicación completa -- el número en sí no necesita cargar
//! significado, mismo criterio que usa `rustc`.

pub struct ErrorCode {
    pub code: &'static str,
    /// Una línea, sin punto final -- mismo criterio que el resto de los
    /// mensajes de este proyecto.
    pub summary: &'static str,
    /// Texto completo que imprime `linkc explain <código>`: ejemplo
    /// resumido de la forma que dispara el error, el arreglo real, y la
    /// sección de GRAMMAR.md con el detalle completo.
    pub explanation: &'static str,
}

pub const CODES: &[ErrorCode] = &[
    ErrorCode {
        code: "L0001",
        summary: "una variante de enum con campos se usó sin llaves",
        explanation: "\
Una variante de enum que declara campos no se puede construir sin ellos --
no hay de dónde inferir sus valores.

    enum Outcome { Good { value: Int }, Bad }
    fn f() -> Outcome { Outcome.Good }              // L0001: falta el valor de 'value'
    fn f() -> Outcome { Outcome.Good { value: 1 } }  // correcto

Una variante SIN campos no tiene este problema -- 'Role.Member' (sin
llaves) es válido, ver L0002 más abajo y GRAMMAR.md §3.209 para la regla
completa.

Ver GRAMMAR.md §3.209.",
    },
    ErrorCode {
        code: "L0002",
        summary: "identificador después de un nombre de enum no nombra ninguna variante real",
        explanation: "\
'Enum.Algo' con 'Algo' mal escrito -- el checker ya sabe que 'Enum' es un
enum real, no una variable, así que el error nombra la variante más
parecida en vez de decir 'variable no declarada'.

    enum Role { Admin, Member }
    fn f() -> Role { Role.Admn }   // L0002: ¿quisiste decir 'Role.Admin'?

Ver GRAMMAR.md §3.209.",
    },
    ErrorCode {
        code: "L0003",
        summary: "un closure lleva una anotación de tipo de retorno, que este lenguaje no acepta",
        explanation: "\
Un closure (`|params| { cuerpo }`) infiere su tipo de retorno del cuerpo
mismo o del `Type::Function` esperado por el contexto que lo recibe --
nunca se anota, a diferencia de `fn`/`rpc`.

    users.find(|u: User| -> Bool { u.active })   // L0003: sobra el '-> Bool'
    users.find(|u: User| { u.active })            // correcto

Ver GRAMMAR.md §3.10 y §3.206.",
    },
    ErrorCode {
        code: "L0004",
        summary: "un valor T? se usó tras un chequeo 'if x != null' esperando que eso angoste el tipo",
        explanation: "\
A diferencia de TypeScript, `if x != null { x.campo }` NO angosta `x` de
`T?` a `T` en este lenguaje -- es deliberado, ver GRAMMAR.md §3.4.
Angostar de verdad necesita `match`:

    fn f(u: User?) -> String {
      if u != null { u.name } else { \"?\" }          // L0004
    }
    fn f(u: User?) -> String {
      match u { v: User => v.name, null => \"?\" }    // correcto -- angosta de verdad
    }

Para el caso común \"dame un default\", `u?.name ?? \"?\"`... no existe
`?.` en este lenguaje, pero si el campo entero es lo que puede faltar,
`x ?? default` alcanza. `x.isSome()`/`x.isNone()` cubren \"solo necesito
saber si hay un valor\", sin desarmarlo.

Ver GRAMMAR.md §3.4 y §3.9.",
    },
    ErrorCode {
        code: "L0005",
        summary: "un closure sin ningún parámetro ('||'), que este lenguaje no soporta todavía",
        explanation: "\
`||` (cero parámetros) no está soportado -- todo closure necesita al
menos 1 parámetro declarado, aunque el cuerpo no lo use.

    xs.forEach(|| { doSomething() })      // L0005
    xs.forEach(|_x| { doSomething() })    // correcto -- un parámetro, sin usarlo

Ver GRAMMAR.md §3.10.",
    },
];

pub fn lookup(code: &str) -> Option<&'static ErrorCode> {
    CODES.iter().find(|c| c.code.eq_ignore_ascii_case(code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_is_well_formed_and_looks_up_by_itself() {
        for c in CODES {
            assert!(c.code.starts_with('L'), "{}", c.code);
            assert!(!c.summary.is_empty(), "{}", c.code);
            assert!(!c.explanation.is_empty(), "{}", c.code);
            assert!(c.explanation.contains("GRAMMAR.md"), "{} no cita ninguna sección de GRAMMAR.md", c.code);
            assert!(lookup(c.code).is_some(), "{} no se encuentra a sí mismo", c.code);
        }
    }

    #[test]
    fn no_two_codes_repeat_the_same_number() {
        let mut seen = std::collections::HashSet::new();
        for c in CODES {
            assert!(seen.insert(c.code), "código repetido: {}", c.code);
        }
    }

    #[test]
    fn lookup_is_case_insensitive_and_rejects_unknown_codes() {
        assert!(lookup("l0001").is_some());
        assert!(lookup("L0001").is_some());
        assert!(lookup("L9999").is_none());
        assert!(lookup("").is_none());
    }
}
