import * as vscode from "vscode";

const SECTION = "luck";

interface Settings {
	enable: boolean;
	"server.path": string;
	"server.extraEnv": Readonly<Record<string, string>>;
	"server.args": readonly string[];
	runFromTemporaryLocation: boolean;
	"statusBar.enable": boolean;
	"statusBar.click": "showOutput" | "restart";
	"trace.extension": boolean;
}

export type SettingKey = keyof Settings;

const DEFAULTS: Settings = {
	enable: true,
	"server.path": "",
	"server.extraEnv": {},
	"server.args": [],
	runFromTemporaryLocation: false,
	"statusBar.enable": true,
	"statusBar.click": "showOutput",
	"trace.extension": false,
};

export function setting<Key extends SettingKey>(key: Key): Settings[Key] {
	return vscode.workspace.getConfiguration(SECTION).get(key, DEFAULTS[key]);
}

export function affectsSetting(event: vscode.ConfigurationChangeEvent, key: SettingKey): boolean {
	return event.affectsConfiguration(`${SECTION}.${key}`);
}
