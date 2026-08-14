# Contributing to VEIL

Thanks for wanting to help. VEIL is alpha; small, well-scoped patches against
[`MISSION.md`](MISSION.md) beat large speculative rewrites.

## License

Contributions are accepted under the **GNU Affero General Public License
v3.0 only** (see [`LICENSE`](LICENSE)). By opening a pull request you agree
that your contribution is licensed inbound = outbound AGPL-3.0-only.

There is **no CLA yet**. That means third-party code cannot be relicensed
into a proprietary commercial edition without a later agreement. The
copyright holder can still dual-license **original** work and sell hosted
VEIL Runtime. If you need a CLA before contributing substantial code,
email jd@unsung-operators.com.

## House rules

1. **Mind Palace first** if you have it configured (`AGENTS.md`).
2. **Do not hand-edit generated customer outputs.** Author `.veil` / `.layer` /
   `.stub`, then `veil gen`. The host (`crates/veil-runtime`, `ui/`) is
   handwritten — edit it directly.
3. **Do not invent a second git.** Commits, branches, merge, log, and diff
   are git’s job. Outstanding changes + sign-off are review state.
4. **No tenant secrets** in the tree (account IDs, IAM ARNs, customer slugs,
   personal paths). Use `.env` (gitignored) and `~/.veil/`.
5. Run what you touched: `cargo test -p <crate>` and `scripts/dev-stack.sh smoke`
   if you changed the host.

## Contact

JD Williams — jd@unsung-operators.com
