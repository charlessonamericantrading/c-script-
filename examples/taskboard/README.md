# Link Realtime Taskboard (Fullstack Demo)

Aplicación Kanban en tiempo real construida de punta a punta con **Link v1.0** en el backend y **React + TypeScript** en el frontend.

## Características
- **Backend Link (`taskboard.link`)**:
  - Modelo de datos con `Task`, `Priority`, `ColumnId`, `Timestamp`, `now()`.
  - Base de datos SQLite automática (`db { tasks: Task[] }`).
  - Streaming SSE reactivo (`stream watchTasks()`).
  - Tests de comportamiento integrados ejecutables con `linkc test`.
- **Frontend React**:
  - Consumo directo de contratos (`contract.d.ts`), cliente tipado (`client.ts`), validadores (`validators.ts`) y hooks generados (`hooks.ts`).
  - Sincronización multi-cliente instantánea mediante Server-Sent Events.

## Cómo ejecutar

1. **Ejecutar las pruebas del backend**:
   ```bash
   linkc test backend/taskboard.link
   ```

2. **Generar los contratos y hooks de TypeScript**:
   ```bash
   linkc build backend/taskboard.link frontend/src/gen
   ```

3. **Iniciar el servidor backend (puerto 8787)**:
   ```bash
   linkc serve backend/taskboard.link 8787
   ```

4. **Iniciar el frontend en otra terminal**:
   ```bash
   cd frontend
   npm install
   npm run dev
   ```
