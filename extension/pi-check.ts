import { homedir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

const REPO_ROOT = fileURLToPath(new URL("..", import.meta.url));
const BINARY = join(process.env.CARGO_HOME ?? join(homedir(), ".cargo"), "bin", "pi-check");
const MINUTE = 60_000;

const USAGE = "Usage: /pi-check [update|install] | /pi-check config <status|apply|capture> [paths]";
const CONFIG_ACTIONS = ["status", "apply", "capture"];

export default function (pi: ExtensionAPI) {
	async function install(): Promise<string | undefined> {
		const result = await pi.exec("cargo", ["install", "--path", REPO_ROOT, "--locked"], { timeout: 10 * MINUTE });
		if (result.code !== 0) return `cargo install failed (exit ${result.code}):\n${result.stderr.trim()}`;
		return undefined;
	}

	function report(title: string, body: string) {
		pi.sendMessage({ customType: "pi-check", content: `**${title}**\n\n\`\`\`\n${body.trimEnd()}\n\`\`\``, display: true }, { triggerTurn: false });
	}

	async function config(args: string[], ctx: ExtensionCommandContext) {
		if (!CONFIG_ACTIONS.includes(args[0] ?? "") || (args[0] !== "capture" && args.length > 1)) {
			ctx.ui.notify(USAGE, "error");
			return;
		}
		// Pi does not expand `~`, so capture paths like ~/.gitignore_global would resolve against the cwd.
		const paths = args.slice(1).map((path) => (path === "~" || path.startsWith("~/") ? homedir() + path.slice(1) : path));
		const result = await pi.exec(BINARY, ["config", args[0], ...paths], { timeout: MINUTE });
		// pi-check prefixes its own errors; anything else means the binary could not run.
		if (result.code !== 0 && result.stdout === "" && !result.stderr.startsWith("pi-check:")) {
			report("pi-check not available", `${result.stderr.trim()}\n\nRun /pi-check install first.`);
			return;
		}
		const output = [result.stdout, result.stderr].filter((s) => s.trim() !== "").join("\n");
		const verdict = result.code === 0 ? (args[0] === "status" ? "in sync" : "done") : "needs attention";
		report(`pi-check config ${args[0]}: ${verdict}`, output);
		if (args[0] === "apply" && result.stdout.includes("Restart Pi")) ctx.ui.notify("Restart Pi to load the applied config.", "warning");
	}

	pi.registerCommand("pi-check", {
		description:
			"Check Pi, packages and Engram against the latest versions (update: update everything, install: rebuild pi-check, config status|apply|capture: sync the tracked config)",
		handler: async (args, ctx) => {
			const parts = args.trim().split(/\s+/).filter((part) => part !== "");
			const sub = parts[0] ?? "";
			if (sub === "config") {
				await config(parts.slice(1), ctx);
				return;
			}
			if (parts.length > 1 || (sub !== "" && sub !== "update" && sub !== "install")) {
				ctx.ui.notify(USAGE, "error");
				return;
			}

			// `update` and `install` always rebuild pi-check from this repo so it stays current.
			if (sub !== "") {
				ctx.ui.notify("pi-check: building and installing into ~/.cargo/bin…", "info");
				const failure = await install();
				if (failure) {
					report("pi-check install failed", failure);
					return;
				}
				if (sub === "install") {
					ctx.ui.notify(`pi-check installed at ${BINARY}`, "info");
					return;
				}
				ctx.ui.notify("pi-check: updating Pi, packages and Engram…", "info");
			}

			const result = await pi.exec(BINARY, sub === "update" ? ["update"] : [], { timeout: 15 * MINUTE });
			if (result.code !== 0 && result.stdout === "" && sub === "") {
				report("pi-check not available", `${result.stderr.trim()}\n\nRun /pi-check install first.`);
				return;
			}
			const output = [result.stdout, result.stderr].filter((s) => s.trim() !== "").join("\n");
			const verdict = result.code === 0 ? "all current" : "needs attention";
			report(`pi-check ${sub || "check"}: ${verdict}`, output);
			if (sub === "update") ctx.ui.notify("Restart Pi to load the updated packages.", "warning");
		},
	});
}
