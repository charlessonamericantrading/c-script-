// decision-errors.ts — RESUELTO (default aplicado, ver GRAMMAR.md §3.5)
//
// Este archivo empezó como una pregunta abierta (A: Result<T,E> vs B: excepción
// tipada). El usuario pidió avanzar con el default recomendado en PLAN.md §8.3.
// Se eligió A — Result<T,E> — porque es la única opción que preserva la tesis
// central del proyecto ("rompe en compilación") para errores: TypeScript no
// tipa lo que se lanza (`catch` siempre es `unknown`), así que una excepción
// hubiera dejado el manejo de errores fuera del contrato verificado por tsc.
// Documentado acá para poder revisar/cambiar si el criterio real difiere.

type ValidationError =
  | { type: "InvalidEmail"; field: string }
  | { type: "TooShort"; field: string; min: number };

type Result<T, E> = { type: "Ok"; value: T } | { type: "Err"; error: E };

type NewUser = { name: string; email: string };

interface UsersClient {
  create(input: NewUser): Promise<Result<{ id: number } & NewUser, ValidationError>>;
}

async function example(usersClient: UsersClient) {
  const input: NewUser = { name: "Ada", email: "not-an-email" };

  const result = await usersClient.create(input);
  if (result.type === "Ok") {
    console.log("creado:", result.value.id);
  } else {
    // exhaustivo: si mañana se agrega un variant a ValidationError,
    // tsc marca este switch como incompleto (GRAMMAR.md §3.3)
    switch (result.error.type) {
      case "InvalidEmail":
        console.error(`email inválido en ${result.error.field}`);
        break;
      case "TooShort":
        console.error(`${result.error.field} necesita mínimo ${result.error.min}`);
        break;
    }
  }
}
