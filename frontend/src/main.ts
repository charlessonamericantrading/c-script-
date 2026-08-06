// Demo E2E (PLAN.md §8.2 / Fase 0): este archivo NUNCA se toca cuando el
// backend cambia. Si `tsc` falla después de recompilar el backend, es la
// prueba de que c-script cumple su promesa central — romper en compilación,
// no en producción.

import { createUsersClient } from "../../gen/client.ts";

async function main() {
  const users = createUsersClient("http://localhost:8787");

  // list -- limit es opcional (tenía default en c-script). Corre antes que
  // nada para que el demo sea correcto tanto en una base vacía (primera
  // corrida) como en una ya persistida por una corrida anterior de este
  // mismo demo (GRAMMAR.md §3.17: `db` sobrevive un restart de `linkc
  // serve`).
  const all = await users.list();
  const firstFive = await users.list(5);
  console.log(`${all.length} usuarios en total, mostrando ${firstFive.length}`);

  // create devuelve Result<User, ValidationError> -- unión discriminada
  // exhaustiva (GRAMMAR.md §3.5): tsc conoce ambos casos por el tag "type".
  const created = await users.create({ name: "Katherine Johnson", email: "katherine@example.com" });
  if (created.type !== "Ok") {
    switch (created.error.type) {
      case "InvalidEmail":
        console.error(`email inválido en el campo '${created.error.field}'`);
        break;
      case "TooShort":
        console.error(`'${created.error.field}' necesita al menos ${created.error.min} caracteres`);
        break;
    }
    return;
  }
  console.log(`Creado: ${created.value.name} (id ${created.value.id})`);

  // getById devuelve User | null (GRAMMAR.md §3.4) -- tsc exige narrowing
  // antes de leer un campo. Usa el id real que `create` acaba de devolver,
  // no un literal hardcodeado: en una base ya persistida por una corrida
  // anterior, el id 1 puede pertenecer a otro usuario, no al que esta
  // corrida acaba de crear.
  const maybeUser = await users.getById(created.value.id);
  if (maybeUser !== null) {
    console.log(`Usuario #${maybeUser.id}: ${maybeUser.name} <${maybeUser.email}> (${maybeUser.role})`);
  }

  // Auth v0 (GRAMMAR.md §3.14): update/remove están detrás de
  // @requires(Role.Admin) -- sin loguearse antes, ambas fallan con 401. El
  // primer usuario creado en una base vacía es Admin (ver `validate` en
  // users.link); en una base ya poblada por una corrida anterior de este
  // demo, el Admin real puede ser un usuario ya existente en vez del que
  // acabamos de crear. Se busca primero en `all` y se usa el recién creado
  // como fallback, para que el demo funcione igual la primera vez que en
  // corridas repetidas contra la misma base.
  const admin = all.find((u) => u.role === "Admin") ?? (created.value.role === "Admin" ? created.value : null);
  if (admin !== null) {
    const token = await users.login(admin.email);
    if (token !== null) {
      users.setToken(token);
    }
  }

  // update -- Patch<User>: solo se manda lo que cambia (GRAMMAR.md §3.4).
  // Actualiza el usuario recién creado, no un id arbitrario.
  const updated = await users.update(created.value.id, { name: "Ada, Condesa de Lovelace" });
  console.log(`Actualizado: ${updated.name}`);

  // remove -- elimina un usuario (requiere rol Admin). 999 es intencional:
  // demuestra el caso "no existe" (devuelve false), no una limpieza real.
  const removed = await users.remove(999);
  console.log(`Eliminado usuario no existente: ${removed}`);
}

main().catch((e) => {
  // Re-lanzar (no solo loguear) es a propósito: sin esto, un demo roto
  // terminaba con código de salida 0 igual, así que un chequeo de CI que
  // solo mirara el exit code nunca hubiera detectado nada -- exactamente
  // el bug real de esta ronda (el propio `main.ts` seguía llamando a
  // `update`/`remove` sin loguearse desde antes de que auth v0 existiera).
  console.error(e);
  throw e;
});
