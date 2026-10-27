import type { ToolContext } from "@opencode/plugin/promise/tool";
import { devicesInputSchema, runInShell, type DevicesInput } from "./common.ts";

/** One Cadmus presence record as printed by `cadmuscli list-devices --json`. */
export type Device = {
  hostname: string;
  addresses: string[];
  version?: string;
  kind?: string;
  platform?: string;
};

/**
 * Parses `cadmuscli list-devices --json`.
 *
 * The command prints an array; an empty network prints `[]`. Anything else is
 * a bug in the caller or a broken build, so fail loudly instead of returning
 * half a device list.
 */
export function parseDevices(output: string): Device[] {
  const parsed: unknown = JSON.parse(output);
  if (!Array.isArray(parsed))
    throw new Error("list-devices did not return an array");
  return parsed.map((entry) => {
    if (typeof entry !== "object" || entry === null) {
      throw new Error("list-devices returned a non-object device");
    }
    const { hostname, addresses } = entry as Partial<Device>;
    if (typeof hostname !== "string" || !Array.isArray(addresses)) {
      throw new Error(
        "list-devices returned a device without hostname or addresses",
      );
    }
    return entry as Device;
  });
}

/** One device per line: address, hostname, then the metadata that is present. */
export function formatDevices(devices: Device[]): string {
  if (devices.length === 0) return "No Cadmus devices found on the network.";
  return devices
    .map((device) => {
      const address = device.addresses[0] ?? "(no address)";
      const meta = [device.platform, device.kind, device.version].filter(
        (field): field is string => typeof field === "string",
      );
      const suffix = meta.length > 0 ? ` (${meta.join(", ")})` : "";
      return `${address}${suffix}\n  ${device.hostname}`;
    })
    .join("\n");
}

async function devices(input: DevicesInput, root: string, signal: AbortSignal) {
  const args = ["cadmuscli", "list-devices", "--json"];
  if (input.timeout !== undefined)
    args.push("--timeout", String(input.timeout));
  const result = await runInShell(root, args, { signal });
  if (result.code !== 0) {
    const body = result.output.trim();
    return `Listing devices failed (exit ${result.code})${body ? `:\n${body}` : "."}`;
  }
  return formatDevices(parseDevices(result.output));
}

export function createDevicesTool(root: string) {
  return {
    name: "devices",
    description: [
      "List Cadmus devices on the local network with their addresses and metadata.",
      "Browses mDNS for two seconds by default; pass a longer timeout when a device is slow to answer.",
      "Prints the first address plus platform (kobo/emulator), build kind and version for each device.",
      "Use it to find the IP address to pass to deploy or restart.",
    ].join(" "),
    input: {
      type: "object",
      properties: {
        timeout: {
          type: "number",
          description: "Seconds to wait for mDNS replies, defaults to 2",
        },
      },
      additionalProperties: false,
    },
    options: {
      namespace: "cadmus.kobo",
      permission: "cadmus.kobo.devices",
    },
    execute: async (input: unknown, context: ToolContext) => {
      const text = await devices(
        devicesInputSchema.parse(input),
        root,
        context.signal,
      );
      return { content: text };
    },
  };
}
