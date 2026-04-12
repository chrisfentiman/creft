# schedule

Schedule recurring agent tasks to run locally on your machine.

## The problem

Claude has scheduled tasks, but they run on Anthropic's servers. You can't point them at local files, local databases, or tools that only exist on your machine. You can't inspect them, modify them, or verify what they did. They run on someone else's infrastructure against a snapshot of your project.

`creft schedule` puts scheduled work on your machine, under your control. Jobs run via macOS launchd, with your shell environment, your filesystem, your tools. They log to a file you can read. They run on a cron schedule you can understand. Nothing leaves your machine.

## Commands

### `creft schedule add <name>`

Install a new scheduled job.

```
# Run a daily brief every morning at 7am
creft schedule add daily-brief \
  --schedule "0 7 * * *" \
  --command "creft run daily-brief" \
  --workdir ~/projects/my-repo

# Run on reboot
creft schedule add startup-sync \
  --schedule @reboot \
  --command "creft sync --all"

# Preview without installing
creft schedule add daily-brief \
  --schedule "0 7 * * *" \
  --command "creft run daily-brief" \
  --preview
```

**Flags:**

| Flag | Description |
|---|---|
| `--schedule` | Cron expression (5 fields) or special string |
| `--command` | Shell command to run |
| `--workdir` | Working directory (defaults to current directory) |
| `--log` | Log file path (defaults to `~/Library/Logs/creft.<name>.log`) |
| `--preview`, `-p` | Print what would be installed without writing anything |

Jobs run as `zsh -lic <command>`, which sources your interactive shell config (`.zshrc`, aliases, `$PATH`). The tools available to you interactively are available to the job.

### `creft schedule ls`

List all creft-managed schedules with status and schedule details.

```
$ creft schedule ls
2 creft-managed schedule(s):

  daily-brief [loaded]
    schedule: 07:00
    command:  creft run daily-brief
    workdir:  /Users/you/projects/my-repo
    log:      /Users/you/Library/Logs/creft.daily-brief.log

  weekly-review [loaded]
    schedule: Sun 09:00
    command:  creft run weekly-review
```

### `creft schedule status <name>`

Show detailed status for one job, including the last 20 lines of its log.

```
creft schedule status daily-brief
```

### `creft schedule run <name>`

Trigger a job immediately, regardless of schedule.

```
creft schedule run daily-brief
```

Useful for testing a new job before waiting for its scheduled time.

### `creft schedule update <name>`

Modify an existing job. Only the flags you pass change; everything else stays as-is.

```
# Change the schedule
creft schedule update daily-brief --schedule "0 8 * * *"

# Change the command
creft schedule update daily-brief --command "creft run daily-brief-v2"
```

The update is atomic: the old job is unloaded, the new plist is written, the new job is loaded. If the load fails, the previous plist is restored.

### `creft schedule remove <name>`

Unload and delete a scheduled job.

```
creft schedule remove daily-brief
```

## Cron schedule syntax

Standard 5-field cron: `minute hour day month weekday`

```
0 7 * * *      every day at 7:00am
0 9 * * 1      every Monday at 9:00am
0 9 * * mon    same (named weekdays accepted)
30 8 1 * *     first of every month at 8:30am
0 8,17 * * *   twice daily at 8am and 5pm
```

**Special strings:**

| String | Equivalent |
|---|---|
| `@hourly` | `0 * * * *` |
| `@daily` | `0 0 * * *` |
| `@weekly` | `0 0 * * 0` |
| `@monthly` | `0 0 1 * *` |
| `@yearly` | `0 0 1 1 *` |
| `@reboot` | Run at login (launchd RunAtLoad) |

Step syntax (`*/N`) is not supported. Write out the explicit values instead: `*/2` in the hour field becomes `0,2,4,6,8,10,12,14,16,18,20,22`.

## How it works

Each job becomes a launchd plist in `~/Library/LaunchAgents/creft.<name>.plist`. The label is `creft.<name>`. Stdout and stderr both go to the log file.

`creft schedule run` calls `launchctl start creft.<name>`, which triggers the job without waiting for its next scheduled time.

`creft schedule status` reads the plist via `plutil` and queries `launchctl list` for the current loaded state.
