import * as vscode from "vscode";
import type { LanguageClientOptions, ServerOptions } from "vscode-languageclient/node";
import { LanguageClient, RevealOutputChannelOn } from "vscode-languageclient/node";
import { discoverServer } from "./discover";
import { logExtension, outputChannel } from "./output";
import { setting } from "./settings";
import type { StatusBar } from "./status-bar";

interface Session {
	readonly client: LanguageClient;
	readonly watcher: vscode.FileSystemWatcher;
}

export class LuckClient implements vscode.Disposable {
	private session: Session | undefined;
	private operation: Promise<void> = Promise.resolve();
	private readonly extensionPath: string;
	private readonly statusBar: StatusBar;

	public constructor(extensionPath: string, statusBar: StatusBar) {
		this.extensionPath = extensionPath;
		this.statusBar = statusBar;
	}

	public get languageClient(): LanguageClient | undefined {
		return this.session?.client;
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

	private enqueue(task: () => Promise<void>): Promise<void> {
		this.operation = this.operation.then(task, task);
		return this.operation;
	}

	private async doStart(): Promise<void> {
		if (this.session) {
			logExtension("start ignored: state is running");
			return;
		}
		this.statusBar.update({ state: "starting" });

		const command = await discoverServer(this.extensionPath);
		if (!command) {
			this.statusBar.update({ state: "error", message: "luck binary not found" });
			void promptMissingBinary();
			return;
		}

		const watcher = vscode.workspace.createFileSystemWatcher("**/{luck.json,.luaurc}");
		const client = createLanguageClient(command, watcher);
		try {
			await client.start();
		} catch (error) {
			watcher.dispose();
			const message = error instanceof Error ? error.message : String(error);
			this.statusBar.update({ state: "error", message });
			logExtension(`start failed: ${message}`);
			void this.reportStartFailure(message);
			return;
		}

		const version = client.initializeResult?.serverInfo?.version;
		this.session = { client, watcher };
		this.statusBar.update({ state: "ready", version });
		logExtension(`start: ready (server ${version ?? "unknown"})`);
	}

	private async doStop(): Promise<void> {
		const { session } = this;
		if (!session) {
			return;
		}
		this.session = undefined;
		try {
			await session.client.stop();
		} catch (error) {
			logExtension(`stop failed: ${error}`);
		}
		session.watcher.dispose();
		this.statusBar.update({ state: "stopped" });
	}

	private async reportStartFailure(message: string): Promise<void> {
		const selection = await vscode.window.showErrorMessage(
			`Luck failed to start: ${message}`,
			"Show Output",
			"Restart",
		);
		if (selection === "Show Output") {
			outputChannel("server").show(true);
		} else if (selection === "Restart") {
			await this.restart();
		}
	}
}

function createLanguageClient(command: string, watcher: vscode.FileSystemWatcher): LanguageClient {
	const args = ["lsp", ...(setting("server.args") ?? [])];
	const env = { ...process.env, ...setting("server.extraEnv") };

	const serverOptions: ServerOptions = {
		run: { command, args, options: { env } },
		debug: { command, args, options: { env: { ...env, RUST_LOG: "debug" } } },
	};

	const clientOptions: LanguageClientOptions = {
		documentSelector: [
			{ scheme: "file", language: "lua" },
			{ scheme: "file", language: "luau" },
			{ scheme: "untitled", language: "lua" },
			{ scheme: "untitled", language: "luau" },
		],
		synchronize: { fileEvents: [watcher] },
		outputChannel: outputChannel("server"),
		traceOutputChannel: outputChannel("trace"),
		revealOutputChannelOn: RevealOutputChannelOn.Never,
	};

	return new LanguageClient("luck", "Luck Language Server", serverOptions, clientOptions);
}

async function promptMissingBinary(): Promise<void> {
	const selection = await vscode.window.showErrorMessage(
		"Luck language server binary not found.",
		"Download Server",
		"Open Settings",
	);
	if (selection === "Download Server") {
		await vscode.commands.executeCommand("luck.downloadServer");
	} else if (selection === "Open Settings") {
		await vscode.commands.executeCommand("workbench.action.openSettings", "luck.server.path");
	}
}
