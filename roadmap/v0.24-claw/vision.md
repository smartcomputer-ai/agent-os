# MiniClaw Vision

The first agent should be a "mini OpenClaw".

I want to be able to define an OpenClaw-like agent with similar top-level prompts as claw and then interact with it through various channels.

## Features
1) talk to the agent in a terminal
2) talk to the agent via WhatsApp
3) talk to the agent via email (forward emails to it), get responses via email
4) have the agent be able to read and edit my calendar
5) have the agent be able to read my emails
6) have the agent do tasks on certain heartbeats/schedules, like check my email, review calendar, etc

## AOS Specific
- communication integrations should be managed by aos workflows and effect families. basically, what is manged by the "Gateway" in Openclaw.

## Open Questions
- how to associate agent sessions with various channels
- how to integrate with things like calendar and external email

## Steps
1) expand aos-cli to chat with an agent through a Codex-like TUI (select/resume sessions, view history, render turn progress, tool chains, compaction, and intervention state)
2) talk to the agent via WA
3) don't plan any further for now
