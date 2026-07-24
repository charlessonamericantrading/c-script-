# __PROJECT_NAME__

Proyecto generado con `linkc new`.

## Empezar

```bash
# 1. Generar el contrato TypeScript desde main.link
linkc build main.link gen

# 2. Confirmar que el frontend tipa limpio
cd frontend && npm install && npx tsc --noEmit

# 3. Levantar el servidor y correr el frontend de verdad
cd .. && linkc serve main.link 8787 &
cd frontend && node src/main.ts
```

(Si `linkc` no está en tu PATH, usá la ruta completa a tu binario compilado, ej. `../c-script/compiler/target/debug/linkc`.)

## Probar el killer feature

Editá `main.link`: renombrá un campo de `Greeting`. Corré `linkc build main.link gen` de nuevo y `npx tsc --noEmit` en `frontend/` **sin tocar `frontend/src/main.ts`**. `tsc` va a fallar en cada línea que usaba ese campo -- ese es el punto central de c-script.

## Desarrollo

`linkc dev main.link gen` reconstruye `gen/contract.d.ts` y `gen/client.ts` automáticamente cada vez que guardás `main.link` (o cualquier archivo que importe).
