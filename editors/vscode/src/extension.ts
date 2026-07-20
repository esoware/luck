import * as vscode from "vscode";
import { LuckClient } from "./client";
import { StatusBar } from "./status-bar";
import { registerCommands } from "./commands";
import { disposeChannels, logExtension } from "./output";

const RESTART_KEYS = [
	"luck.server.path",
	"luck.server.extraEnv",
	"luck.server.args",
	"luck.runFromTemporaryLocation",
];

let client: LuckClient | undefined;
let statusBar: StatusBar | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
	logExtension("activate");

	statusBar = new StatusBar();
	context.subscriptions.push(statusBar);

	client = new LuckClient(context, statusBar);
	context.subscriptions.push(client);

	registerCommands(context, client);
	context.subscriptions.push(watchConfiguration(client, statusBar));

	if (isEnabled()) {
		await client.start();
	}
}

export async function deactivate(): Promise<void> {
	logExtension("deactivate");
	await client?.stop();
	statusBar?.dispose();
	disposeChannels();
}

function isEnabled(): boolean {
	return vscode.workspace.getConfiguration("luck").get<boolean>("enable", true);
}

function watchConfiguration(luckClient: LuckClient, status: StatusBar): vscode.Disposable {
	let restartTimer: NodeJS.Timeout | undefined;
	return vscode.workspace.onDidChangeConfiguration((event) => {
		if (!event.affectsConfiguration("luck")) {
			return;
		}

		if (event.affectsConfiguration("luck.enable")) {
			void (isEnabled() ? luckClient.start() : luckClient.stop());
			return;
		}

		if (RESTART_KEYS.some((key) => event.affectsConfiguration(key))) {
			if (restartTimer) {
				clearTimeout(restartTimer);
			}
			restartTimer = setTimeout(() => void luckClient.restart(), 500);
			return;
		}

		status.refresh();
	});
}
