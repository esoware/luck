import * as vscode from "vscode";
import * as path from "path";
import * as fs from "fs";
import * as os from "os";
import { logExtension } from "./output";

const BIN_BASENAME = process.platform === "win32" ? "luck.exe" : "luck";

/**
 * A null result means no binary was found anywhere; callers respond by
 * offering the "Download Server" command.
 */
export async function discoverServer(
	extensionPath: string,
): Promise<string | null> {
	const config = vscode.workspace.getConfiguration("luck");
	const explicit = config.get<string>("server.path", "").trim();
	if (explicit) {
		if (await isExecutable(explicit)) {
			logExtension(`server: using explicit path ${explicit}`);
			return explicit;
		}
		vscode.window.showWarningMessage(
			`luck.server.path points at ${explicit} but no executable was found there.`,
		);
	}

	const bundled = bundledServerPath(extensionPath);
	if (bundled && (await isExecutable(bundled))) {
		logExtension(`server: using bundled binary ${bundled}`);
		return maybeCopyToTemp(bundled);
	}

	const onPath = await findOnPath(BIN_BASENAME);
	if (onPath) {
		logExtension(`server: using PATH binary ${onPath}`);
		return onPath;
	}

	logExtension("server: no binary found");
	return null;
}

/**
 * On Windows the user often hits "file in use" errors when an extension
 * upgrade overwrites the bundled binary while it's still running. When the
 * `runFromTemporaryLocation` setting is on we copy the binary to a temp
 * directory before launching so the original is free to be replaced.
 */
async function maybeCopyToTemp(source: string): Promise<string> {
	const config = vscode.workspace.getConfiguration("luck");
	if (!config.get<boolean>("runFromTemporaryLocation", false)) {
		return source;
	}
	try {
		const dir = await fs.promises.mkdtemp(path.join(os.tmpdir(), "luck-"));
		const dest = path.join(dir, BIN_BASENAME);
		await fs.promises.copyFile(source, dest);
		if (process.platform !== "win32") {
			await fs.promises.chmod(dest, 0o755);
		}
		logExtension(`server: copied to temp ${dest}`);
		return dest;
	} catch (err) {
		logExtension(`server: temp-copy failed: ${err}`);
		return source;
	}
}

function bundledServerPath(extensionPath: string): string | null {
	const platform = vsixPlatform();
	if (!platform) return null;
	return path.join(extensionPath, "server", platform, BIN_BASENAME);
}

function vsixPlatform(): string | null {
	const arch = process.arch;
	switch (process.platform) {
		case "win32":
			return arch === "arm64" ? "win32-arm64" : "win32-x64";
		case "darwin":
			return arch === "arm64" ? "darwin-arm64" : "darwin-x64";
		case "linux":
			return arch === "arm64" ? "linux-arm64" : "linux-x64";
		default:
			return null;
	}
}

async function isExecutable(p: string): Promise<boolean> {
	try {
		const st = await fs.promises.stat(p);
		return st.isFile();
	} catch {
		return false;
	}
}

async function findOnPath(name: string): Promise<string | null> {
	const pathEnv = process.env["PATH"] ?? "";
	const sep = process.platform === "win32" ? ";" : ":";
	for (const dir of pathEnv.split(sep)) {
		if (!dir) continue;
		const candidate = path.join(dir, name);
		if (await isExecutable(candidate)) {
			return candidate;
		}
	}
	return null;
}

/**
 * Not implemented yet: points the user at manual installation instead of
 * downloading. Registered anyway because the walkthrough and the missing-
 * binary prompt both target this command.
 */
export async function downloadServer(
	context: vscode.ExtensionContext,
): Promise<string | null> {
	const platform = vsixPlatform();
	if (!platform) {
		vscode.window.showErrorMessage(
			`Luck: no prebuilt binary for ${process.platform}/${process.arch}. Install via cargo and set luck.server.path.`,
		);
		return null;
	}
	const version =
		(context.extension.packageJSON?.version as string | undefined) ?? "latest";
	const url = `https://github.com/esoware/luck/releases/download/v${version}/luck-${platform}.zip`;
	vscode.window.showInformationMessage(
		`Luck: download from ${url} not yet implemented. Set luck.server.path or install luck via cargo.`,
	);
	return null;
}
