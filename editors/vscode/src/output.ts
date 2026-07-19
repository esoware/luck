import * as vscode from "vscode";

let serverChannel: vscode.LogOutputChannel | undefined;
let extensionChannel: vscode.OutputChannel | undefined;
let traceChannel: vscode.LogOutputChannel | undefined;

export function serverOutput(): vscode.LogOutputChannel {
	if (!serverChannel) {
		serverChannel = vscode.window.createOutputChannel("Luck Language Server", {
			log: true,
		});
	}
	return serverChannel;
}

export function extensionOutput(): vscode.OutputChannel {
	if (!extensionChannel) {
		extensionChannel = vscode.window.createOutputChannel("Luck Extension");
	}
	return extensionChannel;
}

export function traceOutput(): vscode.LogOutputChannel {
	if (!traceChannel) {
		traceChannel = vscode.window.createOutputChannel("Luck LSP Trace", {
			log: true,
		});
	}
	return traceChannel;
}

export function logExtension(message: string): void {
	const config = vscode.workspace.getConfiguration("luck");
	if (!config.get<boolean>("trace.extension", false)) {
		return;
	}
	const ts = new Date().toISOString();
	extensionOutput().appendLine(`[${ts}] ${message}`);
}

export function disposeChannels(): void {
	serverChannel?.dispose();
	extensionChannel?.dispose();
	traceChannel?.dispose();
	serverChannel = undefined;
	extensionChannel = undefined;
	traceChannel = undefined;
}
