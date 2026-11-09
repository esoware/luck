import * as vscode from "vscode";
import path from "node:path";
import * as fs from "node:fs";
import type { WorkspaceEdit } from "vscode-languageclient/node";
import { version as extensionVersion } from "../package.json";
import type { LuckClient } from "./client";
import { outputChannel } from "./output";

export function registerCommands(client: LuckClient): vscode.Disposable[] {
	const handlers = {
		"luck.restart": () => client.restart(),
		"luck.start": () => client.start(),
		"luck.stop": () => client.stop(),
		"luck.showServerOutput": () => outputChannel("server").show(true),
		"luck.showExtensionOutput": () => outputChannel("extension").show(true),
		"luck.showTraceOutput": () => outputChannel("trace").show(true),
		"luck.serverVersion": () => showServerVersion(client),
		"luck.formatDocument": () => vscode.commands.executeCommand("editor.action.formatDocument"),
		"luck.formatSelection": () => vscode.commands.executeCommand("editor.action.formatSelection"),
		"luck.applyAllFixesFile": applyAllFixesFile,
		"luck.applyAllFixesWorkspace": () => applyAllFixesWorkspace(client),
		"luck.createConfig": createConfig,
		"luck.downloadServer": async () => {
			await vscode.env.openExternal(
				vscode.Uri.parse("https://github.com/esoware/luck#quick-start"),
			);
		},
		"luck.viewSyntaxTree": () => viewSyntaxTree(client),
		"luck.copyDebugInfo": () => copyDebugInfo(client),
		"luck.reportIssue": async () => {
			await vscode.env.openExternal(vscode.Uri.parse("https://github.com/esoware/luck/issues/new"));
		},
		"luck.openWalkthrough": () =>
			vscode.commands.executeCommand(
				"workbench.action.openWalkthrough",
				"luck.luck#luck.gettingStarted",
				false,
			),
	} satisfies Record<string, () => unknown>;

	return Object.entries(handlers).map(([command, handler]) =>
		vscode.commands.registerCommand(command, handler),
	);
}

function showServerVersion(client: LuckClient): void {
	const serverInfo = client.languageClient?.initializeResult?.serverInfo;
	void vscode.window.showInformationMessage(
		`${serverInfo?.name ?? "luck"} ${serverInfo?.version ?? "unknown"}`,
	);
}

async function applyAllFixesFile(): Promise<void> {
	if (!vscode.window.activeTextEditor) {
		return;
	}
	await vscode.commands.executeCommand("editor.action.sourceAction", {
		kind: "source.fixAll.luck",
		apply: "first",
	});
}

async function applyAllFixesWorkspace(client: LuckClient): Promise<void> {
	const { languageClient } = client;
	if (!languageClient) {
		void vscode.window.showWarningMessage("Luck: server is not running.");
		return;
	}
	try {
		const fixes = await languageClient.sendRequest<WorkspaceEdit>("luck/fixAllWorkspace", {});
		const edit = await languageClient.protocol2CodeConverter.asWorkspaceEdit(fixes);
		if (edit.size === 0) {
			void vscode.window.showInformationMessage("Luck: no workspace fixes to apply.");
		} else if (await vscode.workspace.applyEdit(edit)) {
			void vscode.window.showInformationMessage("Luck: workspace fixes applied.");
		} else {
			void vscode.window.showErrorMessage("Luck: workspace fixes could not be applied.");
		}
	} catch (error) {
		void vscode.window.showErrorMessage(`Luck: ${error}`);
	}
}

async function createConfig(): Promise<void> {
	const folder = vscode.workspace.workspaceFolders?.[0];
	if (!folder) {
		void vscode.window.showErrorMessage("Luck: open a folder before creating a config.");
		return;
	}
	const configPath = path.join(folder.uri.fsPath, "luck.json");
	if (fs.existsSync(configPath)) {
		void vscode.window.showWarningMessage("Luck: luck.json already exists.");
	} else {
		const skeleton = {
			lua: "lua54",
			luau: "luau",
			lint: {
				extra_globals: [],
			},
		};
		await fs.promises.writeFile(configPath, `${JSON.stringify(skeleton, null, 2)}\n`, "utf8");
	}
	await vscode.window.showTextDocument(vscode.Uri.file(configPath));
}

async function viewSyntaxTree(client: LuckClient): Promise<void> {
	const { languageClient } = client;
	const editor = vscode.window.activeTextEditor;
	if (!languageClient || !editor) {
		void vscode.window.showWarningMessage("Luck: open a Lua file first.");
		return;
	}
	try {
		const tree = await languageClient.sendRequest<string>("luck/syntaxTree", {
			textDocument: { uri: editor.document.uri.toString() },
		});
		const document = await vscode.workspace.openTextDocument({
			content: tree,
			language: "luck-syntax-tree",
		});
		await vscode.window.showTextDocument(document, vscode.ViewColumn.Beside);
	} catch (error) {
		void vscode.window.showErrorMessage(`Luck: ${error}`);
	}
}

async function copyDebugInfo(client: LuckClient): Promise<void> {
	const serverVersion = client.languageClient?.initializeResult?.serverInfo?.version;
	const lines = [
		`Luck extension: ${extensionVersion}`,
		`Luck server:    ${serverVersion ?? "unknown"}`,
		`VS Code:        ${vscode.version}`,
		`Platform:       ${process.platform}/${process.arch}`,
		`Node:           ${process.version}`,
	];
	await vscode.env.clipboard.writeText(lines.join("\n"));
	void vscode.window.showInformationMessage("Luck: debug info copied.");
}
