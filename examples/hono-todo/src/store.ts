import { randomUUID } from "node:crypto";

export interface Todo {
  id: string;
  title: string;
  done: boolean;
  createdAt: string;
}

// In-memory store. Data is lost on restart.
const todos = new Map<string, Todo>();

export function listTodos(done?: boolean): Todo[] {
  const all = [...todos.values()];
  return done === undefined ? all : all.filter((t) => t.done === done);
}

export function getTodo(id: string): Todo | undefined {
  return todos.get(id);
}

export function findByTitle(title: string): Todo | undefined {
  const needle = title.trim().toLowerCase();
  return [...todos.values()].find((t) => t.title.toLowerCase() === needle);
}

export function createTodo(title: string): Todo {
  const todo: Todo = {
    id: randomUUID(),
    title: title.trim(),
    done: false,
    createdAt: new Date().toISOString(),
  };
  todos.set(todo.id, todo);
  return todo;
}

export function updateTodo(id: string, patch: Partial<Pick<Todo, "title" | "done">>): Todo | undefined {
  const current = todos.get(id);
  if (!current) return undefined;
  const next = { ...current, ...patch };
  todos.set(id, next);
  return next;
}

export function deleteTodo(id: string): boolean {
  return todos.delete(id);
}

export function todoStats(): { total: number; done: number; open: number } {
  const all = [...todos.values()];
  const done = all.filter((t) => t.done).length;
  return { total: all.length, done, open: all.length - done };
}
