import * as vscode from "vscode";
import { LuckClient } from "./client";
import { registerCommands } from "./commands";
import { disposeChannels, logExtension } from "./output";
import { affectsSetting, setting } from "./settings";
import { StatusBar } from "./status-bar";

let client: LuckClient | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
	logExtension("activate");

	const statusBar = new StatusBar();
	client = new LuckClient(context, statusBar);
	context.subscriptions.push(
		statusBar,
		...registerCommands(client),
		vscode.workspace.onDidOpenTextDocument((document) => {
			if (setting("treatLuaFilesAsLuau")) {
				void switchLuaFile(document, "lua", "luau");
			}
		}),
		vscode.workspace.onDidChangeConfiguration((event) => {
			if (!affectsSetting(event, "treatLuaFilesAsLuau")) {
				return;
			}
			if (setting("treatLuaFilesAsLuau")) {
				switchOpenLuaFiles("lua", "luau");
			} else {
				switchOpenLuaFiles("luau", "lua");
			}
		}),
	);
	if (setting("treatLuaFilesAsLuau")) {
		switchOpenLuaFiles("lua", "luau");
	}

	if (setting("enable")) {
		await client.start();
	}
}

export async function deactivate(): Promise<void> {
	logExtension("deactivate");
	await client?.shutdown();
	disposeChannels();
}

function switchOpenLuaFiles(from: string, to: string): void {
	for (const document of vscode.workspace.textDocuments) {
		void switchLuaFile(document, from, to);
	}
}

async function switchLuaFile(
	document: vscode.TextDocument,
	from: string,
	to: string,
): Promise<void> {
	if (document.languageId !== from || !document.uri.path.toLowerCase().endsWith(".lua")) {
		return;
	}
	try {
		await vscode.languages.setTextDocumentLanguage(document, to);
	} catch (error) {
		logExtension(`switching ${document.uri.toString()} to ${to} failed: ${error}`);
	}
}
