// decision-nullability.ts — RESUELTO (default aplicado, ver GRAMMAR.md §3.4)
//
// Este archivo empezó como una pregunta abierta ("¿cómo llamarías vos a
// updateUser?"). El usuario pidió avanzar con el default recomendado en
// PLAN.md §8.3 en vez de escribirlo a mano. Queda documentado acá para que
// se pueda revisar y cambiar más adelante si el criterio real difiere —
// no es una decisión irreversible, es un punto de partida razonado.
//
// La regla (JSON Merge Patch / RFC 7386, y el patrón habitual de inputs
// nullable en GraphQL):
//   - clave ausente en el patch  -> no tocar el campo
//   - clave presente con valor   -> fijar el campo a ese valor
//   - clave presente con `null`  -> limpiar el campo (solo si es nullable
//                                    en la base, es decir, declarado T? en c-script)

type User = {
  id: number;
  name: string;
  bio?: string;              // opcional al CREAR -> clave ausente = "opcional al crear"
  deletedAt: string | null;  // nullable siempre -> clave siempre presente, null hasta borrarse
};

// Patch<User> generado por c-script: todos los campos vuelven `?:`, preservando
// si además eran nullable (`| null`) en la base.
type PatchUser = {
  name?: string;
  bio?: string;              // no era T? en la base -> no se puede "limpiar", solo fijar u omitir
  // deletedAt no aparece: no tiene sentido "parchear" un campo de solo-lectura
  // gestionado por el propio backend (borrado). Si hiciera falta, se modela
  // como T? explícito y entonces sí entraría como `deletedAt?: string | null`.
};

declare function updateUser(id: number, patch: PatchUser): Promise<User>;

// Cambiar solo el nombre, sin tocar bio:
updateUser(42, { name: "Ada" });

// Fijar bio (antes ausente):
updateUser(42, { bio: "Matemática y programadora" });
