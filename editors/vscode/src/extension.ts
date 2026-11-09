import type * as vscode from "vscode";
import { LuckClient } from "./client";
import { registerCommands } from "./commands";
import { disposeChannels, logExtension } from "./output";
import { setting } from "./settings";
import { StatusBar } from "./status-bar";

let client: LuckClient | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
	logExtension("activate");

	const statusBar = new StatusBar();
	client = new LuckClient(context.extensionPath, statusBar);
	context.subscriptions.push(statusBar, ...registerCommands(client));

	if (setting("enable")) {
		await client.start();
	}
}

export async function deactivate(): Promise<void> {
	logExtension("deactivate");
	await client?.shutdown();
	disposeChannels();
}
