import * as vscode from "vscode";
import { LuckClient } from "./client";
import { StatusBar } from "./statusBar";
import { registerCommands } from "./commands";
import { disposeChannels, logExtension } from "./output";

let client: LuckClient | undefined;
let statusBar: StatusBar | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
	logExtension("activate");

	statusBar = new StatusBar();
	context.subscriptions.push(statusBar);

	client = new LuckClient(context, statusBar);
	context.subscriptions.push(client);

	registerCommands(context, client);

	let restartTimer: NodeJS.Timeout | undefined;
	context.subscriptions.push(
		vscode.workspace.onDidChangeConfiguration((event) => {
			if (!event.affectsConfiguration("luck")) return;
			const needsRestart = [
				"luck.server.path",
				"luck.server.extraEnv",
				"luck.server.args",
				"luck.runFromTemporaryLocation",
			].some((key) => event.affectsConfiguration(key));
			if (!needsRestart) {
				statusBar?.update({ state: "ready" });
				return;
			}
			if (restartTimer) clearTimeout(restartTimer);
			restartTimer = setTimeout(() => client?.restart(), 500);
		}),
	);

	const config = vscode.workspace.getConfiguration("luck");
	if (config.get<boolean>("enable", true)) {
		await client.start();
	}
}

export async function deactivate(): Promise<void> {
	logExtension("deactivate");
	await client?.stop();
	statusBar?.dispose();
	disposeChannels();
}
