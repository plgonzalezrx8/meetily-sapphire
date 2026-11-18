# Contributing to Meetily Sapphire

Thank you for contributing to Meetily Sapphire! Current application work belongs in `frontend/` and its Rust/Tauri core; `backend/` is an unsupported legacy archive.

## Before You Start

> [!IMPORTANT]
> Before starting contribution work, and again before proposing publication, search relevant [upstream issues](https://github.com/Zackriya-Solutions/meetily/issues?q=is%3Aissue) and [upstream PRs](https://github.com/Zackriya-Solutions/meetily/pulls?q=is%3Apr), as well as [fork PRs](https://github.com/plgonzalezrx8/meetily-sapphire/pulls?q=is%3Apr). The fork does not currently use GitHub issues. Search the underlying problem, related components, and alternative wording, not just the proposed title.
>
> Use an existing issue when it covers the problem; an issue without an implementation is still an opportunity to contribute. If a PR already implements the same change, coordinate with its author before starting a competing contribution. Discuss a materially different approach before proceeding separately.

- Discuss substantial features, architectural changes, and large refactors with the fork maintainers before implementation. Straightforward bug fixes and documentation corrections can go directly to a focused PR without creating an issue just for the PR.
- Read any issue or PR supplied for the work, including its discussion and linked contributions.
- Keep each PR focused on one problem. Similar symptoms may have different causes: compare reproduction steps, affected versions, and platforms before treating reports as duplicates. Report related links and explain what overlaps or differs.
- If GitHub access is unavailable, state that the duplicate search was not completed. Do not claim that no related work exists. Local work can continue with this limitation made explicit.
- Local-only edits and investigations do not require a GitHub search. Do not turn an investigation into speculative issues or overlapping PRs.
- If you use a coding agent, have it read [AGENTS.md](AGENTS.md). A new agent session is not a reason to open another issue.

## AI Assistance

> [!IMPORTANT]
> AI-assisted code and writing are welcome. You are responsible for reviewing, understanding, and verifying everything you submit, including factual claims and test results. Never include private prompts, meeting content, or credentials in submissions.
>
> Agents need explicit user authorization for each action: committing, pushing, opening issues or PRs, or posting comments. Permission to edit locally is not permission to publish. Before authorizing publication, review the related issue/PR links, a concise explanation of the change, verification results, and known limitations.

## Development Workflow

### Branch Strategy

- `Development` is the integration branch for feature work.
- `main` is the release branch.
- Create feature branches from `Development` and target `Development` for integration. It is currently local only, so integration happens through local merges until it is published. Do not target `main` for feature contributions.

### Getting Started

1. Fork and clone [Meetily Sapphire](https://github.com/plgonzalezrx8/meetily-sapphire).
2. Use the maintained local `Development` branch as your base. While it is unpublished, coordinate access to that branch with the maintainers.
3. Create a focused feature branch from `Development`.
4. Follow the [build guide](docs/BUILDING.md), including the first-run sidecar setup.

### Development Process

1. Start from the appropriate fork base described above
2. Create a new branch for each feature/fix
3. Make your changes
4. Write or update tests as needed
5. Run relevant checks and record any platform or hardware limitations
6. Update documentation if necessary

### Reporting problems

The fork does not currently use GitHub issues. Coordinate fork-specific reports with its maintainers. For an upstream problem, check the [upstream issue forms](https://github.com/Zackriya-Solutions/meetily/issues/new/choose) and their contribution requirements before reporting it there.

For bugs, provide the app version, OS and architecture, installation method, reproduction steps, and expected versus actual behavior. Include relevant settings, redacted logs, or screenshots where useful. Build-tool versions are only needed when relevant to a source-build problem.

> [!WARNING]
> Do not publish private recordings, transcripts, credentials, or unredacted logs. Use synthetic examples or redact sensitive information before attaching files.

### Pull Request Process

1. Integrate the focused feature branch into local `Development` through a local merge while that branch is unpublished. Once published, open a PR targeting `Development`.
2. Link related issues or discussions when applicable. Link overlapping PRs and explain any distinct approach; do not create duplicate work silently.
3. Complete the [PR template](.github/pull_request_template.md), including your duplicate search and verification results.
4. Review your own changes, run relevant local checks, and report their results.
5. Request maintainer review and address feedback.
6. Keep the work up to date with `Development`. A maintainer merges it after approval.

### Submission Templates

The [PR template](.github/pull_request_template.md) provides the fork’s submission format. Templates provide prompts; maintainers still assess scope, evidence, and possible duplicates.

## Code Style

- Read the relevant source and follow existing patterns and code style
- Avoid unrelated cleanup and unnecessary dependencies
- Use meaningful variable and function names
- Add comments for complex logic
- Keep functions small and focused
- Write clear commit messages

## Commit Message Format

```
<type>(<scope>): <subject>

<body>

<footer>
```

Types:

- feat: New feature
- fix: Bug fix
- docs: Documentation changes
- style: Code style changes
- refactor: Code refactoring
- test: Adding/updating tests
- chore: Maintenance tasks

## Testing

- Add or update regression coverage for changed behavior where practical.
- Run checks relevant to the affected code and exercise the changed path.
- State the commands, platform, and observed results in the PR, along with checks you could not run. Do not claim a build, test, or hardware check passed without running it.
- Documentation-only changes do not need application tests; check their links and any changed instructions or configuration.

## Documentation

- Update documentation for new features
- Update the README in the same change when setup, features, or providers change. Once available, add links to the published `Development` branch and fork installers.
- Document API changes
- Add comments for complex code

## Review Process

1. PRs require at least one review
2. Address all review comments
3. Keep the PR up to date with the target branch
4. Squash commits if requested

## Getting Help

- Ask usage and setup questions in the [Meetily community chat](https://discord.gg/crRymMQBFH).
- Use the [development resources](#development-resources) below for development questions.
- Coordinate fork-specific bug reports, feature requests, and documentation problems with fork maintainers.

## Development Resources

- [Build instructions](docs/BUILDING.md)
- [Linux build instructions](docs/building_in_linux.md)
- [GPU acceleration guide](docs/GPU_ACCELERATION.md)
- [Architecture overview](docs/architecture.md)
- [PR template](.github/pull_request_template.md)


## License

By contributing, you agree that your contributions will be licensed under the project's MIT License.
