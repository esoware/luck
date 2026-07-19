import * as vscode from "vscode";
import {
	LanguageClient,
	LanguageClientOptions,
	ServerOptions,
	RevealOutputChannelOn,
} from "vscode-languageclient/node";
import { discoverServer } from "./discover";
import { serverOutput, traceOutput, logExtension } from "./output";
import { StatusBar } from "./statusBar";

export class LuckClient implements vscode.Disposable {
	private client: LanguageClient | undefined;
	private context: vscode.ExtensionContext;
	private status: StatusBar;

	constructor(context: vscode.ExtensionContext, status: StatusBar) {
		this.context = context;
		this.status = status;
	}

	get languageClient(): LanguageClient | undefined {
		return this.client;
	}

	async start(): Promise<void> {
		if (this.client) {
			logExtension("client.start: already running");
			return;
		}
		this.status.update({ state: "starting" });
		const command = await discoverServer(this.context.extensionPath);
		if (!command) {
			this.status.update({
				state: "error",
				message: "luck binary not found",
			});
			vscode.window
				.showErrorMessage(
					"Luck language server binary not found.",
					"Download Server",
					"Open Settings",
				)
				.then((selection) => {
					if (selection === "Download Server") {
						vscode.commands.executeCommand("luck.downloadServer");
					} else if (selection === "Open Settings") {
						vscode.commands.executeCommand(
							"workbench.action.openSettings",
							"luck.server.path",
						);
					}
				});
			return;
		}

		const config = vscode.workspace.getConfiguration("luck");
		const extraEnv =
			config.get<Record<string, string>>("server.extraEnv", {}) ?? {};
		const extraArgs = config.get<string[]>("server.args", []) ?? [];

		const baseArgs = ["lsp", ...extraArgs];
		// No `transport` here: vscode-languageclient appends `--stdio` to the
		// args when TransportKind.stdio is declared, which `luck lsp` rejects.
		// An Executable without a transport already talks over stdio pipes.
		const serverOptions: ServerOptions = {
			run: {
				command,
				args: baseArgs,
				options: { env: { ...process.env, ...extraEnv } },
			},
			debug: {
				command,
				args: baseArgs,
				options: { env: { ...process.env, ...extraEnv, RUST_LOG: "debug" } },
			},
		};

		const clientOptions: LanguageClientOptions = {
			documentSelector: [
				{ scheme: "file", language: "lua" },
				{ scheme: "file", language: "luau" },
				{ scheme: "untitled", language: "lua" },
				{ scheme: "untitled", language: "luau" },
			],
			synchronize: {
				fileEvents: [
					vscode.workspace.createFileSystemWatcher(
						"**/{luck.json,.luaurc}",
					),
				],
			},
			outputChannel: serverOutput(),
			traceOutputChannel: traceOutput(),
			revealOutputChannelOn: RevealOutputChannelOn.Never,
		};

		this.client = new LanguageClient(
			"luck",
			"Luck Language Server",
			serverOptions,
			clientOptions,
		);

		try {
			await this.client.start();
			const version =
				this.client.initializeResult?.serverInfo?.version ?? undefined;
			this.status.update({ state: "ready", version });
			logExtension(`client.start: ready (server ${version ?? "unknown"})`);
		} catch (err) {
			this.client = undefined;
			const message = err instanceof Error ? err.message : String(err);
			this.status.update({ state: "error", message });
			logExtension(`client.start: failed: ${message}`);
			vscode.window.showErrorMessage(`Luck: ${message}`);
		}
	}

	async stop(): Promise<void> {
		if (!this.client) return;
		try {
			await this.client.stop();
		} catch (err) {
			logExtension(`client.stop: ${err}`);
		}
		this.client = undefined;
		this.status.update({ state: "stopped" });
	}

	async restart(): Promise<void> {
		await this.stop();
		await this.start();
	}

	dispose(): void {
		void this.stop();
	}
}
