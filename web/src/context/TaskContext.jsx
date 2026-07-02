import { createContext, useContext, useState, useEffect, useCallback } from 'react';
import useWebSocket from '../hooks/useWebSocket';
import { getTasks, getActiveTasks, getTask } from '../api/client';

const TaskContext = createContext(null);

function upsertTask(list, task) {
  const idx = list.findIndex(t => t.id === task.id);
  if (idx >= 0) {
    const updated = [...list];
    updated[idx] = task;
    return updated;
  }
  return [task, ...list].slice(0, 50);
}

export function TaskProvider({ children }) {
  const [tasks, setTasks] = useState([]);
  const [activeTasks, setActiveTasks] = useState([]);
  const [isOpen, setIsOpen] = useState(false);
  const [selectedTaskId, setSelectedTaskId] = useState(null);
  const [selectedTaskSteps, setSelectedTaskSteps] = useState([]);

  // Load initial tasks
  useEffect(() => {
    getTasks({ limit: 30 }).then(res => {
      if (res.data?.tasks) setTasks(res.data.tasks);
    }).catch(() => {});
    getActiveTasks().then(res => {
      if (Array.isArray(res.data)) setActiveTasks(res.data);
    }).catch(() => {});
  }, []);

  // WebSocket handler
  useWebSocket({
    'task:update': (data) => {
      if (!data?.task) return;
      const { task, steps } = data;

      setTasks(prev => upsertTask(prev, task));

      if (task.status === 'pending' || task.status === 'running') {
        setActiveTasks(prev => upsertTask(prev, task));
      } else {
        setActiveTasks(prev => prev.filter(t => t.id !== task.id));
      }

      // Update selected task steps if viewing this task
      if (task.id === selectedTaskId && steps) {
        setSelectedTaskSteps(steps);
      }
    }
  });

  const selectTask = useCallback(async (taskId) => {
    setSelectedTaskId(taskId);
    if (taskId) {
      try {
        const res = await getTask(taskId);
        if (res.data?.steps) setSelectedTaskSteps(res.data.steps);
      } catch { /* ignore */ }
    } else {
      setSelectedTaskSteps([]);
    }
  }, []);

  const value = {
    tasks,
    activeTasks,
    activeCount: activeTasks.length,
    isOpen,
    setIsOpen,
    selectedTaskId,
    selectedTaskSteps,
    selectTask,
  };

  return (
    <TaskContext.Provider value={value}>
      {children}
    </TaskContext.Provider>
  );
}

export function useTaskContext() {
  const context = useContext(TaskContext);
  if (!context) {
    throw new Error('useTaskContext must be used within a TaskProvider');
  }
  return context;
}

export default TaskContext;
