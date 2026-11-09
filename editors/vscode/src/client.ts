import * as vscode from "vscode";
import type { LanguageClientOptions, ServerOptions } from "vscode-languageclient/node";
import { LanguageClient, RevealOutputChannelOn } from "vscode-languageclient/node";
import { discoverServer } from "./discover";
import { logExtension, outputChannel } from "./output";
import { affectsSetting, setting } from "./settings";
import type { SettingKey } from "./settings";
import type { StatusBar } from "./status-bar";

const RESTART_SETTINGS = [
	"server.path",
	"server.extraEnv",
	"server.args",
	"runFromTemporaryLocation",
] as const satisfies readonly SettingKey[];

const RESTART_DEBOUNCE_MS = 500;

export class LuckClient {
	private client: LanguageClient | undefined;
	private operation: Promise<void> = Promise.resolve();
	private restartTimer: NodeJS.Timeout | undefined;
	private readonly extensionPath: string;
	private readonly statusBar: StatusBar;
	private readonly settingsListener = vscode.workspace.onDidChangeConfiguration((event) => {
		if (affectsSetting(event, "enable")) {
			void (setting("enable") ? this.start() : this.stop());
			return;
		}
		if (RESTART_SETTINGS.some((key) => affectsSetting(event, key))) {
			clearTimeout(this.restartTimer);
			this.restartTimer = setTimeout(() => void this.restart(), RESTART_DEBOUNCE_MS);
		}
	});

	public constructor(extensionPath: string, statusBar: StatusBar) {
		this.extensionPath = extensionPath;
		this.statusBar = statusBar;
	}

	public get languageClient(): LanguageClient | undefined {
		return this.client;
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

	public shutdown(): Promise<void> {
		clearTimeout(this.restartTimer);
		this.settingsListener.dispose();
		return this.stop();
	}

	private enqueue(task: () => Promise<void>): Promise<void> {
		this.operation = this.operation.then(task).catch((error: unknown) => {
			void this.reportStartFailure(error);
		});
		return this.operation;
	}

	private async doStart(): Promise<void> {
		if (this.client) {
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

		const client = createLanguageClient(command);
		await client.start();
		const version = client.initializeResult?.serverInfo?.version;
		this.client = client;
		this.statusBar.update({ state: "ready", version });
		logExtension(`start: ready (server ${version ?? "unknown"})`);
	}

	private async doStop(): Promise<void> {
		const { client } = this;
		if (!client) {
			return;
		}
		this.client = undefined;
		try {
			await client.stop();
		} catch (error) {
			logExtension(`stop failed: ${error}`);
		}
		this.statusBar.update({ state: "stopped" });
	}

	private async reportStartFailure(error: unknown): Promise<void> {
		const message = error instanceof Error ? error.message : String(error);
		this.statusBar.update({ state: "error", message });
		logExtension(`start failed: ${message}`);
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

function createLanguageClient(command: string): LanguageClient {
	const args = ["lsp", ...(setting("server.args") ?? [])];
	const env = { ...process.env, ...setting("server.extraEnv") };

	const serverOptions: ServerOptions = { command, args, options: { env } };

	const clientOptions: LanguageClientOptions = {
		documentSelector: [
			{ scheme: "file", language: "lua" },
			{ scheme: "file", language: "luau" },
			{ scheme: "untitled", language: "lua" },
			{ scheme: "untitled", language: "luau" },
		],
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
