use anyhow::Result;
use crossterm::{
    cursor::{MoveTo, RestorePosition, SavePosition},
    execute,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};
use std::io::{self, Write};

pub struct Progress {
    total: usize,
    finished: usize,
    enabled: bool,
}

impl Progress {
    pub fn new(total: usize) -> Self {
        Self {
            total,
            finished: 0,
            enabled: terminal::size().is_ok(),
        }
    }

    pub fn set_finished(&mut self, finished: usize) -> Result<()> {
        self.finished = finished.min(self.total);
        self.draw()
    }

    pub fn increment(&mut self) -> Result<()> {
        if self.finished < self.total {
            self.finished += 1;
        }

        self.draw()
    }

    pub fn log_build(&mut self, target: &str) -> Result<()> {
        self.clear()?;

        let mut stdout = io::stdout();

        execute!(
            stdout,
            SetForegroundColor(Color::Green),
            Print("[build]"),
            ResetColor,
            Print(format!(" {}\n", target)),
        )?;

        stdout.flush()?;
        self.draw()
    }

    pub fn log_done(&mut self, target: &str) -> Result<()> {
        self.clear()?;

        let mut stdout = io::stdout();

        execute!(
            stdout,
            SetForegroundColor(Color::Blue),
            Print("[done]"),
            ResetColor,
            Print(format!(" {}\n", target)),
        )?;

        stdout.flush()?;
        self.draw()
    }

    pub fn log_skip(&mut self, target: &str) -> Result<()> {
        self.clear()?;

        let mut stdout = io::stdout();

        execute!(stdout, Print(format!("[skip] {}\n", target)),)?;

        stdout.flush()?;
        self.draw()
    }

    pub fn log_fail(&mut self, target: &str) -> Result<()> {
        self.clear()?;

        let mut stdout = io::stdout();

        execute!(stdout, Print(format!("[fail] {}\n", target)),)?;

        stdout.flush()?;
        self.draw()
    }

    pub fn clear(&self) -> Result<()> {
        if !self.enabled || self.total == 0 {
            return Ok(());
        }

        let (_, rows) = terminal::size()?;

        if rows == 0 {
            return Ok(());
        }

        let mut stdout = io::stdout();

        execute!(
            stdout,
            SavePosition,
            MoveTo(0, rows - 1),
            Clear(ClearType::CurrentLine),
            RestorePosition,
        )?;

        stdout.flush()?;

        Ok(())
    }

    pub fn draw(&self) -> Result<()> {
        if self.total == 0 {
            return Ok(());
        }

        if !self.enabled {
            return Ok(());
        }

        let (columns, rows) = terminal::size()?;

        if rows == 0 || columns == 0 {
            return Ok(());
        }

        let counter = format!(" ({}/{})", self.finished, self.total);

        let reserved = counter.len() + 3;

        let width = (columns as usize).saturating_sub(reserved).max(10);

        let filled = if self.total == 0 {
            0
        } else {
            self.finished * width / self.total
        };

        let mut bar = String::with_capacity(width);

        for index in 0..width {
            if index < filled {
                bar.push('=');
            } else if index == filled && self.finished < self.total {
                bar.push('>');
            } else {
                bar.push(' ');
            }
        }

        let mut stdout = io::stdout();

        execute!(
            stdout,
            SavePosition,
            MoveTo(0, rows - 1),
            Clear(ClearType::CurrentLine),
            Print("["),
            Print(bar),
            Print("]"),
            Print(counter),
            RestorePosition,
        )?;

        stdout.flush()?;

        Ok(())
    }

    pub fn finish(&self) -> Result<()> {
        self.clear()
    }
}
