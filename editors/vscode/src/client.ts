import * as vscode from "vscode";
import type { LanguageClientOptions, ServerOptions } from "vscode-languageclient/node";
import { LanguageClient, RevealOutputChannelOn } from "vscode-languageclient/node";
import { discoverServer } from "./discover";
import { serverOutput, traceOutput, logExtension } from "./output";
import type { StatusBar } from "./status-bar";

type State =
	| { tag: "stopped" }
	| { tag: "starting" }
	| { tag: "running"; client: LanguageClient }
	| { tag: "stopping" };

export class LuckClient implements vscode.Disposable {
	private state: State = { tag: "stopped" };
	private clientScope: vscode.Disposable[] = [];
	private operation: Promise<void> = Promise.resolve();
	private readonly context: vscode.ExtensionContext;
	private readonly status: StatusBar;

	public constructor(context: vscode.ExtensionContext, status: StatusBar) {
		this.context = context;
		this.status = status;
	}

	public get languageClient(): LanguageClient | undefined {
		return this.state.tag === "running" ? this.state.client : undefined;
	}

	public start(): Promise<void> {
		return this.enqueue(() => this.doStart());
	}

	public stop(): Promise<void> {
		return this.enqueue(() => this.doStop());
	}

	public restart(): Promise<void> {
		return this.enqueue(async () => {
			await this.doStop();
			await this.doStart();
		});
	}

	public dispose(): void {
		void this.stop();
	}

	// Lifecycle transitions run one at a time, so start/stop/restart can never
	// Interleave and observe a half-built client.
	private enqueue(task: () => Promise<void>): Promise<void> {
		this.operation = this.operation.then(task, task);
		return this.operation;
	}

	private async doStart(): Promise<void> {
		if (this.state.tag !== "stopped") {
			logExtension(`start ignored: state is ${this.state.tag}`);
			return;
		}
		this.state = { tag: "starting" };
		this.status.update({ state: "starting" });

		const command = await discoverServer(this.context.extensionPath);
		if (!command) {
			this.state = { tag: "stopped" };
			this.status.update({ state: "error", message: "luck binary not found" });
			this.promptMissingBinary();
			return;
		}

		const client = this.buildClient(command);
		try {
			await client.start();
		} catch (error) {
			this.disposeClientScope();
			this.state = { tag: "stopped" };
			const message = error instanceof Error ? error.message : String(error);
			this.status.update({ state: "error", message });
			logExtension(`start failed: ${message}`);
			this.reportStartFailure(message);
			return;
		}

		const version = client.initializeResult?.serverInfo?.version ?? undefined;
		this.state = { tag: "running", client };
		this.status.update({ state: "ready", version });
		logExtension(`start: ready (server ${version ?? "unknown"})`);
	}

	private async doStop(): Promise<void> {
		if (this.state.tag !== "running") {
			return;
		}
		const { client } = this.state;
		this.state = { tag: "stopping" };
		try {
			await client.stop();
		} catch (error) {
			logExtension(`stop: ${error}`);
		}
		this.disposeClientScope();
		this.state = { tag: "stopped" };
		this.status.update({ state: "stopped" });
	}

	private buildClient(command: string): LanguageClient {
		const config = vscode.workspace.getConfiguration("luck");
		const extraEnv = config.get<Record<string, string>>("server.extraEnv", {}) ?? {};
		const extraArgs = config.get<string[]>("server.args", []) ?? [];
		const args = ["lsp", ...extraArgs];

		// No `transport` here: vscode-languageclient appends `--stdio` to the
		// Args when TransportKind.stdio is declared, which `luck lsp` rejects.
		// An Executable without a transport already talks over stdio pipes.
		const serverOptions: ServerOptions = {
			run: {
				command,
				args,
				options: { env: { ...process.env, ...extraEnv } },
			},
			debug: {
				command,
				args,
				options: { env: { ...process.env, ...extraEnv, RUST_LOG: "debug" } },
			},
		};

		const watcher = vscode.workspace.createFileSystemWatcher("**/{luck.json,.luaurc}");
		this.clientScope.push(watcher);

		const clientOptions: LanguageClientOptions = {
			documentSelector: [
				{ scheme: "file", language: "lua" },
				{ scheme: "file", language: "luau" },
				{ scheme: "untitled", language: "lua" },
				{ scheme: "untitled", language: "luau" },
			],
			synchronize: { fileEvents: [watcher] },
			outputChannel: serverOutput(),
			traceOutputChannel: traceOutput(),
			revealOutputChannelOn: RevealOutputChannelOn.Never,
		};

		return new LanguageClient("luck", "Luck Language Server", serverOptions, clientOptions);
	}

	private disposeClientScope(): void {
		for (const disposable of this.clientScope) {
			disposable.dispose();
		}
		this.clientScope = [];
	}

	private promptMissingBinary(): void {
		void vscode.window
			.showErrorMessage(
				"Luck language server binary not found.",
				"Download Server",
				"Open Settings",
			)
			.then((selection) => {
				if (selection === "Download Server") {
					void vscode.commands.executeCommand("luck.downloadServer");
				} else if (selection === "Open Settings") {
					void vscode.commands.executeCommand("workbench.action.openSettings", "luck.server.path");
				}
			});
	}

	private reportStartFailure(message: string): void {
		void vscode.window
			.showErrorMessage(`Luck failed to start: ${message}`, "Show Output", "Restart")
			.then((selection) => {
				if (selection === "Show Output") {
					serverOutput().show(true);
				} else if (selection === "Restart") {
					void this.restart();
				}
			});
	}
}
