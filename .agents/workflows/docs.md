---
description: managing project documentation
---
# Documentation Workflow

This workflow ensures all pedagogical, technical, and reproduction documents for `scrcpyrust` remain up-to-date.

## 1. Documentation Map
- **teaching_kids.md**: Simplified, analogy-driven guide for beginners/children.
- **PRD.md**: Formal product requirements and feature status.
- **SSO.md**: Software system overview and architectural diagrams.
- **recreation_prompt.md**: Master prompt for AI-assisted project regeneration.
- **braindump.md**: Technical knowledge base (protocols, FFmpeg, SDL2).
- **historyprompt.md**: Full project evolution and reasoning log.

## 2. Maintenance Steps
1. **Feature Update**: When adding a new feature, update the **PRD.md** status and add a technical note to **braindump.md**.
2. **Architecture Change**: If modifying core modules (e.g., ADB, Media), update the Mermaid diagrams in **SSO.md**.
3. **Internal Release**: After a major milestone, update **historyprompt.md** with a summary of the session's decisions and challenges.
4. **Kid-Friendly Sync**: Ensure any major new "lego pieces" are explained with a new analogy in **teaching_kids.md**.

## 3. Review Process
- Verify all relative file links in markdown headers.
- Use `cargo doc --open` to ensure internal crate documentation is also current.
