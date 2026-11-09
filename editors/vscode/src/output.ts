import * as vscode from "vscode";
import { setting } from "./settings";

const CHANNEL_NAMES = {
	server: "Luck Language Server",
	extension: "Luck Extension",
	trace: "Luck LSP Trace",
} as const satisfies Record<string, string>;

type ChannelId = keyof typeof CHANNEL_NAMES;

const channels = new Map<ChannelId, vscode.LogOutputChannel>();

export function outputChannel(id: ChannelId): vscode.LogOutputChannel {
	let channel = channels.get(id);
	if (!channel) {
		channel = vscode.window.createOutputChannel(CHANNEL_NAMES[id], { log: true });
		channels.set(id, channel);
	}
	return channel;
}

export function logExtension(message: string): void {
	if (setting("trace.extension")) {
		outputChannel("extension").info(message);
	}
}

export function disposeChannels(): void {
	for (const channel of channels.values()) {
		channel.dispose();
	}
	channels.clear();
}
