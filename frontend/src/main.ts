// Demo E2E (PLAN.md §8.2 / Fase 0): este archivo NUNCA se toca cuando el
// backend cambia. Si `tsc` falla después de recompilar el backend, es la
// prueba de que c-script cumple su promesa central — romper en compilación,
// no en producción.

import { createUsersClient } from "../../gen/client.ts";

async function main() {
  const users = createUsersClient("http://localhost:8787");

  // getById devuelve User | null (GRAMMAR.md §3.4) -- tsc exige narrowing
  // antes de leer un campo.
  const maybeUser = await users.getById(1);
  if (maybeUser !== null) {
    console.log(`Usuario #${maybeUser.id}: ${maybeUser.name} <${maybeUser.email}> (${maybeUser.role})`);
  }

  // list -- limit es opcional (tenía default en c-script)
  const all = await users.list();
  const firstFive = await users.list(5);
  console.log(`${all.length} usuarios en total, mostrando ${firstFive.length}`);

  // create devuelve Result<User, ValidationError> -- unión discriminada
  // exhaustiva (GRAMMAR.md §3.5): tsc conoce ambos casos por el tag "type".
  const created = await users.create({ name: "Katherine Johnson", email: "katherine@example.com" });
  if (created.type === "Ok") {
    console.log(`Creado: ${created.value.name} (id ${created.value.id})`);
  } else {
    switch (created.error.type) {
      case "InvalidEmail":
        console.error(`email inválido en el campo '${created.error.field}'`);
        break;
      case "TooShort":
        console.error(`'${created.error.field}' necesita al menos ${created.error.min} caracteres`);
        break;
    }
  }

  // update -- Patch<User>: solo se manda lo que cambia (GRAMMAR.md §3.4)
  const updated = await users.update(1, { name: "Ada, Condesa de Lovelace" });
  console.log(`Actualizado: ${updated.name}`);

  // remove -- elimina un usuario (requiere rol Admin)
  const removed = await users.remove(999);
  console.log(`Eliminado usuario no existente: ${removed}`);
}


main().catch((e) => {
  console.error(e);
});
