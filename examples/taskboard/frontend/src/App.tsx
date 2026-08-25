import React, { useState, useMemo } from 'react';
import { createTasksClient } from './gen/client.ts';
import type { Task, Priority, ColumnId, NewTask } from './gen/contract.d.ts';
import { useTasksListQuery, useTasksCreateMutation, useTasksWatchTasks } from './gen/hooks.ts';

const client = createTasksClient('http://localhost:8787');

export function App() {
  const { data: initialTasks, loading, refetch } = useTasksListQuery(client);
  const { mutate: createTask, loading: creating } = useTasksCreateMutation(client);
  const { data: streamEvents, isConnected } = useTasksWatchTasks(client);

  const [title, setTitle] = useState('');
  const [description, setDescription] = useState('');
  const [priority, setPriority] = useState<Priority>('Medium');
  const [column, setColumn] = useState<ColumnId>('Todo');

  // Unir lista inicial con mutaciones recibidas del stream SSE reactivo
  const tasks = useMemo(() => {
    const map = new Map<number, Task>();
    if (initialTasks) {
      for (const t of initialTasks) {
        map.set(t.id, t);
      }
    }
    if (streamEvents) {
      for (const t of streamEvents) {
        map.set(t.id, t);
      }
    }
    return Array.from(map.values());
  }, [initialTasks, streamEvents]);

  const todoTasks = tasks.filter((t) => t.column === 'Todo');
  const inProgressTasks = tasks.filter((t) => t.column === 'InProgress');
  const doneTasks = tasks.filter((t) => t.column === 'Done');

  async function handleCreate(e: React.FormEvent) {
    e.preventDefault();
    if (!title.trim()) return;

    const input: NewTask = {
      title: title.trim(),
      description: description.trim() || null,
      priority,
      column,
      assigneeEmail: null,
    };

    // `mutate` nunca relanza (GRAMMAR.md §3.128) -- devuelve `null` en el
    // fallo, ya reflejado en `error` del hook; sin este chequeo, un fallo
    // de creación limpiaría el formulario igual, como si hubiera salido bien.
    const created = await createTask(input);
    if (!created) return;
    setTitle('');
    setDescription('');
    refetch();
  }

  async function handleMove(id: number, targetCol: ColumnId) {
    await client.update(id, { column: targetCol });
    refetch();
  }

  async function handleDelete(id: number) {
    await client.remove(id);
    refetch();
  }

  return (
    <div className="container">
      <header>
        <div>
          <h1>⚡ Link Taskboard</h1>
          <p style={{ color: '#94a3b8', fontSize: '0.9rem', marginTop: '4px' }}>
            Fullstack TypeScript End-to-End con Streaming SSE & SQLite Nativo
          </p>
        </div>
        <div className="live-indicator">
          <span className="live-dot"></span>
          {isConnected ? 'Stream en Vivo Conectado' : 'Conectando Stream...'}
        </div>
      </header>

      {/* Resumen de Estadísticas */}
      <div className="stats-bar">
        <div className="stat-card">
          <div className="stat-val">{tasks.length}</div>
          <div className="stat-lbl">Total de Tareas</div>
        </div>
        <div className="stat-card">
          <div className="stat-val" style={{ color: '#cbd5e1' }}>{todoTasks.length}</div>
          <div className="stat-lbl">Por Hacer (Todo)</div>
        </div>
        <div className="stat-card">
          <div className="stat-val" style={{ color: '#fbbf24' }}>{inProgressTasks.length}</div>
          <div className="stat-lbl">En Progreso</div>
        </div>
        <div className="stat-card">
          <div className="stat-val" style={{ color: '#34d399' }}>{doneTasks.length}</div>
          <div className="stat-lbl">Completadas (Done)</div>
        </div>
      </div>

      {/* Formulario de Creación */}
      <div className="create-form">
        <form onSubmit={handleCreate} className="form-row">
          <input
            type="text"
            placeholder="Título de la tarea..."
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            required
          />
          <input
            type="text"
            placeholder="Descripción (opcional)..."
            value={description}
            onChange={(e) => setDescription(e.target.value)}
          />
          <select value={priority} onChange={(e) => setPriority(e.target.value as Priority)}>
            <option value="Low">Prioridad Baja</option>
            <option value="Medium">Prioridad Media</option>
            <option value="High">Prioridad Alta</option>
          </select>
          <select value={column} onChange={(e) => setColumn(e.target.value as ColumnId)}>
            <option value="Todo">Todo</option>
            <option value="InProgress">In Progress</option>
            <option value="Done">Done</option>
          </select>
          <button type="submit" className="primary" disabled={creating}>
            {creating ? 'Creando...' : '+ Agregar'}
          </button>
        </form>
      </div>

      {/* Columnas Kanban */}
      {loading ? (
        <p style={{ textAlign: 'center', color: '#94a3b8' }}>Cargando tablero...</p>
      ) : (
        <div className="board-grid">
          {/* Columna: Todo */}
          <div className="column">
            <div className="col-header">
              <span>📋 Por Hacer</span>
              <span className="badge badge-todo">{todoTasks.length}</span>
            </div>
            {todoTasks.map((t) => (
              <TaskCard key={t.id} task={t} onMove={handleMove} onDelete={handleDelete} />
            ))}
          </div>

          {/* Columna: In Progress */}
          <div className="column">
            <div className="col-header">
              <span>🚀 En Progreso</span>
              <span className="badge badge-inprogress">{inProgressTasks.length}</span>
            </div>
            {inProgressTasks.map((t) => (
              <TaskCard key={t.id} task={t} onMove={handleMove} onDelete={handleDelete} />
            ))}
          </div>

          {/* Columna: Done */}
          <div className="column">
            <div className="col-header">
              <span>✅ Completadas</span>
              <span className="badge badge-done">{doneTasks.length}</span>
            </div>
            {doneTasks.map((t) => (
              <TaskCard key={t.id} task={t} onMove={handleMove} onDelete={handleDelete} />
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function TaskCard({
  task,
  onMove,
  onDelete,
}: {
  task: Task;
  onMove: (id: number, col: ColumnId) => void;
  onDelete: (id: number) => void;
}) {
  const priorityBadge =
    task.priority === 'High' ? 'badge-high' : task.priority === 'Medium' ? 'badge-med' : 'badge-low';

  return (
    <div className="task-card">
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start' }}>
        <div className="task-title">{task.title}</div>
        <span className={`badge ${priorityBadge}`}>{task.priority}</span>
      </div>
      {task.description && <div className="task-desc">{task.description}</div>}
      <div className="task-footer">
        <span>#{task.id} • {new Date(task.createdAt).toLocaleTimeString()}</span>
      </div>
      <div className="actions">
        {task.column !== 'Todo' && (
          <button onClick={() => onMove(task.id, 'Todo')}>← Todo</button>
        )}
        {task.column !== 'InProgress' && (
          <button onClick={() => onMove(task.id, 'InProgress')}>⚡ Proceso</button>
        )}
        {task.column !== 'Done' && (
          <button onClick={() => onMove(task.id, 'Done')}>✓ Done</button>
        )}
        <button className="danger" onClick={() => onDelete(task.id)} style={{ marginLeft: 'auto' }}>
          ✕
        </button>
      </div>
    </div>
  );
}
