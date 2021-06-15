mod uniq_id;

use anyhow::{Context, Result};
use colored::Colorize;
// use crossterm::style::Stylize;
use crossterm::{cursor, style, terminal, ExecutableCommand};
use std::alloc::System;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::io::Stdout;
use std::io::{stdout, Write};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;
use uniq_id::UniqID;

#[derive(Clone, Default)]
pub struct Status(Arc<RwLock<StatusInternal>>);

#[derive(Default)]
pub struct StatusInternal {
    current_height: usize,
    tasks_internal: BTreeMap<UniqID, TaskInternal>,
}

pub struct TaskInternal {
    name: String,
    id: UniqID,
    started_at: SystemTime,
    status: TaskStatus,
    subtasks: BTreeSet<UniqID>,
}

enum TaskStatus {
    Running,
    Finished(TaskResult, SystemTime),
}

enum TaskResult {
    Success,
    Failure,
}

pub struct Task(UniqID, Status);

impl Status {
    pub fn new() -> Self {
        let status = Self::default();

        let s = status.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                s.print().await.ok();
            }
        });
        status
    }
    async fn spawn<F, FT, T>(&self, name: &str, f: F) -> Result<T>
    where
        F: FnOnce(Task) -> FT,
        FT: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        self.spawn_internal(name, f, None).await
    }

    async fn spawn_internal<F, FT, T>(&self, name: &str, f: F, parent: Option<UniqID>) -> Result<T>
    where
        F: FnOnce(Task) -> FT,
        FT: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let mut status = self.0.write().await;
        let task_internal = TaskInternal::new(name);
        let id = task_internal.id;

        if let Some(parent) = parent {
            let parent = status
                .tasks_internal
                .get_mut(&parent)
                .context("parent must be there")?;
            parent.subtasks.insert(id);
        }

        let task = task_internal.task(self);
        status.tasks_internal.insert(id, task_internal);
        drop(status);
        let result = tokio::spawn(f(task)).await?;
        let mut status = self.0.write().await;

        let task_internal = status
            .tasks_internal
            .get_mut(&id)
            .context("Task must be present in the tree")?;

        let tast_status = match result.is_ok() {
            true => TaskResult::Success,
            false => TaskResult::Failure,
        };
        task_internal.status = TaskStatus::Finished(tast_status, SystemTime::now());

        result.with_context(|| format!("Failed to execute task `{}`", name))
    }

    async fn print(&self) -> Result<()> {
        let mut status = self.0.write().await;

        let mut task_to_parent = BTreeMap::new();
        for (id, t) in &status.tasks_internal {
            for subtask_id in &t.subtasks {
                task_to_parent
                    .entry(subtask_id)
                    .or_insert_with(BTreeSet::new)
                    .insert(id);
            }
        }

        type Depth = usize;
        let mut stack: Vec<(UniqID, Depth)> = status
            .tasks_internal
            .keys()
            .filter(|id| !task_to_parent.contains_key(id))
            .map(|id| (*id, 0))
            .collect();

        let mut rows = vec![];
        while let Some((id, depth)) = stack.pop() {
            let task = status.tasks_internal.get(&id).context("must be present")?;
            rows.push(self.task_row(task, depth)?);
            for subtask_id in &task.subtasks {
                stack.push((*subtask_id, depth + 1));
            }
        }

        let height = rows.len();

        self.clear(status.current_height)?;
        status.current_height = height;

        let mut stdout = stdout();
        for row in rows {
            stdout.execute(style::Print(row))?;
        }

        Ok(())
    }

    fn task_row(&self, task_internal: &TaskInternal, depth: usize) -> Result<String> {
        let indent = "  ".repeat(depth);
        let status = match task_internal.status {
            TaskStatus::Running => "[ RUNS ]".black().on_yellow(),
            TaskStatus::Finished(TaskResult::Success, _) => "[  OK  ]".black().on_green(),
            TaskStatus::Finished(TaskResult::Failure, _) => "[ FAIL ]".on_red(),
        };

        let duration = match task_internal.status {
            TaskStatus::Finished(_, finished_at) => {
                finished_at.duration_since(task_internal.started_at)
            }
            _ => task_internal.started_at.elapsed(),
        }?;

        let indent_len = depth * 2;
        Ok(format!(
            "{}{} {}{}{:?}\n",
            indent,
            status,
            task_internal.name,
            " ".repeat(50 - indent_len), // spacer
            duration,
        ))
        //
    }

    fn clear(&self, height: usize) -> Result<()> {
        let mut stdout = stdout();

        for _ in 0..height {
            stdout.execute(cursor::MoveUp(1))?;
            stdout.execute(terminal::Clear(terminal::ClearType::CurrentLine))?;
        }

        Ok(())
    }
}

impl TaskInternal {
    fn new<S: Into<String>>(s: S) -> Self {
        Self {
            status: TaskStatus::Running,
            name: s.into(),
            id: UniqID::new(),
            started_at: SystemTime::now(),
            subtasks: BTreeSet::new(),
        }
    }

    fn task(&self, status: &Status) -> Task {
        Task(self.id, status.clone())
    }
}

impl Task {
    async fn spawn<F, FT, T>(&self, name: &str, f: F) -> Result<T>
    where
        F: FnOnce(Task) -> FT,
        FT: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        self.1.spawn_internal(name, f, Some(self.0)).await
    }
}

#[tokio::main]
async fn main() {
    let status = Status::new();

    status
        .spawn("task_1", |task| async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

            tokio::join!(
                task.spawn("task_2", |task| async move {
                    tokio::time::sleep(tokio::time::Duration::from_millis(11000)).await;
                    Ok(())
                }),
                task.spawn("task_3", |task| async move {
                    tokio::time::sleep(tokio::time::Duration::from_millis(2750)).await;

                    task.spawn("task_4", |task| async move {
                        tokio::time::sleep(tokio::time::Duration::from_millis(3200)).await;
                        Ok(())
                    })
                    .await?;
                    tokio::time::sleep(tokio::time::Duration::from_millis(2750)).await;
                    Ok(())
                }),
            );

            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
            Ok(())
        })
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    // let mut stdout = stdout();
    // print(&mut stdout, "hey bro\n");
    // print(&mut stdout, "hey bro\n");
    // print(&mut stdout, "hey bro\n");
    // cursor_up(&mut stdout);
    // erase_line(&mut stdout);
    // print(&mut stdout, "hey bro2\n");

    // stdout.flush().unwrap();
}

fn print<D: std::fmt::Display>(stdout: &mut Stdout, d: D) {
    stdout.execute(style::Print(d)).unwrap();
}

fn erase_line(stdout: &mut Stdout) {
    stdout
        .execute(terminal::Clear(terminal::ClearType::CurrentLine))
        .unwrap();
}

fn cursor_up(stdout: &mut Stdout) {
    stdout.execute(cursor::MoveUp(1)).unwrap();
}
