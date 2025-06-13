# ClipSync Parallel Agent Coordination Guide

## Overview
This guide helps you manage multiple Claude agents working in parallel on the ClipSync project. Each sprint has 3 agents working on different aspects simultaneously.

## Setting Up Parallel Agents

### Step 1: Start Three Claude Conversations
Open three separate Claude conversations (browser tabs/windows):
- Agent 1: Project Setup / Foundation
- Agent 2: Core Modules / Features  
- Agent 3: Data Layer / Specialized Components

### Step 2: Initialize Each Agent
Copy the entire content of the respective agent prompt file to each conversation:
- Tab 1: `sprint1_agent1_project_setup.md`
- Tab 2: `sprint1_agent2_core_modules.md`
- Tab 3: `sprint1_agent3_data_layer.md`

### Step 3: Provide Context
After the initial prompt, provide each agent with:
1. Path to the ClipSync specification: `@ai_docs/ClipSync_Spec.md`
2. Path to their specific task files
3. Current working directory
4. Any completed work from dependencies

## Managing Dependencies

### Sprint 1 Dependencies
```
Agent 1 (Project Setup)
    ↓
    ├─→ Agent 2 (Core Modules)
    └─→ Agent 3 (Data Layer)
```

- Agent 1 must complete first (creates project structure)
- Agents 2 & 3 can work in parallel after Agent 1

### Coordination Points

#### After Agent 1 Completes:
1. Verify project builds: `cargo build`
2. Share the created structure with Agents 2 & 3
3. Give both agents the green light to proceed

#### During Development:
- Agents 2 & 3 should define their public APIs early
- Share interface definitions between agents if needed
- Use stub implementations for missing dependencies

#### Integration Points:
- Agent 2's config module may define paths used by Agent 3
- Both must follow the module structure from Agent 1
- Coordinate on shared types (e.g., error types)

## Communication Protocol

### Status Updates
Have each agent provide regular updates:
```
STATUS: Starting task X
STATUS: Defined API for module Y  
STATUS: Completed implementation of Z
STATUS: All tests passing
```

### Sharing Code
When agents need to share interfaces:
```
INTERFACE DEFINITION:
```rust
pub trait ClipboardProvider {
    // ... trait definition
}
```
PLEASE SHARE WITH: Agent 3
```

### Handling Conflicts
If two agents need to modify the same file:
1. Identify the conflict early
2. Decide which agent owns the file
3. Have one agent define the interface
4. Other agent implements against the interface

## Sprint Progression

### Completing Sprint 1
Before moving to Sprint 2:
- [ ] All three agents report completion
- [ ] Run full test suite: `cargo test`
- [ ] Verify cross-compilation: `make build-all`
- [ ] Check no merge conflicts
- [ ] Review integration points

### Starting Sprint 2
1. Ensure Sprint 1 is fully integrated
2. Start three new conversations for Sprint 2
3. Provide Sprint 2 agent prompts
4. Include summary of Sprint 1 completion

## Best Practices

### 1. Regular Synchronization
- Check in with each agent every 10-15 minutes
- Share important decisions across agents
- Identify blockers early

### 2. Documentation
Have each agent create:
- API documentation for public interfaces
- Integration notes for other agents
- Any discovered platform-specific issues

### 3. Testing Coordination
- Agent 1: Project-level test infrastructure
- Agents 2 & 3: Module-specific tests
- Integration tests after all complete

### 4. Error Handling
Establish common error handling patterns:
- Use consistent error types
- Follow same logging conventions
- Coordinate on error propagation

## Troubleshooting

### Agent is Blocked
1. Identify the specific dependency
2. Check if another agent can provide a stub
3. Reorder tasks if possible
4. Work on tests/docs while waiting

### Integration Failures
1. Verify all agents used same project structure
2. Check for API mismatches
3. Ensure consistent dependency versions
4. Run clippy and fmt on all code

### Performance Issues
1. Each agent should benchmark their module
2. Share performance constraints
3. Coordinate on optimization strategies

## Quality Checklist

Before considering a sprint complete:
- [ ] All code compiles without warnings
- [ ] All tests pass
- [ ] Code passes `cargo clippy`
- [ ] Code is formatted with `cargo fmt`
- [ ] APIs are documented
- [ ] Integration points tested
- [ ] Platform-specific code works
- [ ] No security vulnerabilities

## Notes for Future Sprints

### Sprint 2 (Networking)
- Agent 1 must define auth interface first
- Agents 2 & 3 can mock auth for testing
- Coordinate on network error types

### Sprint 3 (Integration)
- Requires all Sprint 1 & 2 work complete
- Agents need tight coordination
- Focus on end-to-end testing

### Sprint 4 (Release)
- Can be highly parallel
- Coordinate version numbers
- Ensure consistent documentation

Remember: Communication and coordination are key to successful parallel development!