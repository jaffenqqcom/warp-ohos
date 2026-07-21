---
name: bugfix-report
description: Use when documenting a completed bug fix to generate a structured bugfix report with root cause analysis and modification record
---

# Bugfix Report

## Overview

Create a structured markdown bugfix report that records the complete debugging journey: problem description, symptoms, root cause analysis, solution, and all modified files. Reports are saved to a specified directory for future reference.

## When to Use

- After identifying and fixing a bug
- When the debugging process involved multiple attempts or dead ends
- Before moving on to the next task, to preserve debugging context
- When the fix touches platform-specific code (OHOS, Windows, macOS, etc.)
- When the root cause was non-obvious and future agents might benefit

**Do NOT use for:**
- Trivial one-line fixes with obvious causes
- Changes documented entirely in git commit messages
- Features or enhancements (use design docs instead)

## Output Format

Each report is a single markdown file written to the specified directory. The filename follows the convention `YYYY-MM-DD-<kebab-case-slug>.md`.

## Required Fields

Every report MUST contain these sections in order:

```
# <Bug Title>

## 问题描述 (Problem Description)
What was the bug? When did it occur? One paragraph.

## 问题表现 (Symptoms)
How did the bug manifest? Bullet list of observable symptoms:
- Error messages, crash logs, unexpected behavior
- Steps to reproduce
- Frequency or conditions

## 问题原因 (Root Cause)
Why did the bug happen? Include the chain of causation:
- The mistaken assumption, missing edge case, or API misuse
- Relevant code paths and why they failed
- Diagrams or ascii art if helpful for spatial/coordinate bugs

## 解决方案 (Solution)
What was the fix? How was it implemented:
- The key insight that led to the fix
- Before/after code comparison for critical changes
- Why this approach was chosen over alternatives

## 修改文件 (Modified Files)
List every file that was changed, with a one-line summary per file:

- path/to/file.rs — what changed and why (e.g. "fixed coordinate flip: y_up to y_down")
```

## Workflow

1. Determine the target directory (passed as argument or prompted)
2. Collect all information from the conversation history and code changes
3. Generate the report filename as `<target-dir>/YYYY-MM-DD-<slug>.md`
4. Write the report with all five required sections
5. If the fix is OHOS-specific, add a reference to `[[ohos-debug-lessons]]` in the report

## Common Mistakes

- **Omitting dead ends**: Record what was tried and failed
- **Vague root cause**: "Missing coordinate conversion" is not enough — specify which coordinate system and the exact missing transformation
- **Missing file paths**: Every changed file must be listed with the specific change
- **Skipping symptoms**: Without clear symptoms, a report is useless for future pattern matching
- **Writing for the current self**: Write as if a future agent knows nothing about this bug
