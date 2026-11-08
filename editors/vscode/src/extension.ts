import * as vscode from "vscode";
import { LuckClient } from "./client";
import { registerCommands } from "./commands";
import { disposeChannels, logExtension } from "./output";
import { affectsSetting, setting } from "./settings";
import type { SettingKey } from "./settings";
import { StatusBar } from "./status-bar";

const RESTART_SETTINGS = [
	"server.path",
	"server.extraEnv",
	"server.args",
	"runFromTemporaryLocation",
] as const satisfies readonly SettingKey[];

let client: LuckClient | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
	logExtension("activate");

	const statusBar = new StatusBar();
	client = new LuckClient(context.extensionPath, statusBar);
	const { version } = context.extension.packageJSON as { version?: string };
	context.subscriptions.push(
		statusBar,
		client,
		...registerCommands(client, version),
		watchServerSettings(client),
	);

	if (setting("enable")) {
		await client.start();
	}
}

export async function deactivate(): Promise<void> {
	logExtension("deactivate");
	await client?.stop();
	disposeChannels();
}

function watchServerSettings(luckClient: LuckClient): vscode.Disposable {
	let restartTimer: NodeJS.Timeout | undefined;
	const listener = vscode.workspace.onDidChangeConfiguration((event) => {
		if (affectsSetting(event, "enable")) {
			void (setting("enable") ? luckClient.start() : luckClient.stop());
			return;
		}
		if (RESTART_SETTINGS.some((key) => affectsSetting(event, key))) {
			clearTimeout(restartTimer);
			restartTimer = setTimeout(() => void luckClient.restart(), 500);
		}
	});
	return new vscode.Disposable(() => {
		clearTimeout(restartTimer);
		listener.dispose();
	});
}
