import type { Meta, StoryObj } from "@storybook/nextjs-vite";
import { userEvent, within } from "storybook/test";
import type {
  LogDownloadEvent,
  LogFile,
  LogLine,
  LogPage,
  LogReadRequest,
  LogSummary,
} from "@/lib/companion";
import { CompanionLogs, type CompanionLogsApi } from "./index";

const NOW = "2026-11-16T07:00:00Z";

const currentRun: LogFile = {
  id: "01a20edc-8083-75d7-b900-35f14f330c68",
  kind: "run",
  name: "cadmus-01a20edc-8083-75d7-b900-35f14f330c68.json",
  size: 744_003_200,
  modified: NOW,
  started: "2026-11-16T06:59:00Z",
  current: true,
  runId: "01a20edc-8083-75d7-b900-35f14f330c68",
};

const olderRun: LogFile = {
  id: "01a1f426-9a23-7673-b3c4-b840e4ad2e1a",
  kind: "run",
  name: "cadmus-01a1f426-9a23-7673-b3c4-b840e4ad2e1a.json",
  size: 12_345_678,
  modified: "2026-11-15T07:00:00Z",
  started: "2026-11-15T07:00:00Z",
  current: false,
  runId: "01a1f426-9a23-7673-b3c4-b840e4ad2e1a",
};

const launcherLog: LogFile = {
  id: "info",
  kind: "launcher",
  name: "info.log",
  size: 1_204_224,
  modified: "2026-11-16T06:00:00Z",
  started: "",
  current: false,
  runId: "",
};

const sampleLines = [
  JSON.stringify({
    timestamp: "2026-11-16T07:00:00.000000Z",
    level: "INFO",
    fields: { message: "Cadmus started with ID test" },
    target: "cadmus_core::app",
  }),
  "cadmus.sh: launcher output that is not JSON",
  JSON.stringify({
    timestamp: "2026-11-16T07:00:02.000000Z",
    level: "DEBUG",
    fields: {
      message: "processing in state Cell { value: Data }",
      "log.target": "xml5ever::tokenizer",
      "log.line": 1000,
    },
    target: "xml5ever::tokenizer",
  }),
  JSON.stringify({
    timestamp: "2026-11-16T07:00:05.000000Z",
    level: "WARN",
    fields: { message: "slow import", path: "/mnt/onboard/Books/X.epub" },
    target: "cadmus_core::library",
  }),
  JSON.stringify({
    timestamp: "2026-11-16T07:00:09.000000Z",
    level: "ERROR",
    fields: { message: "Wi-Fi state changed to connected", attempt: 3 },
    target: "cadmus_core::device::wifi",
  }),
];

const fullPage: LogPage = {
  lines: sampleLines.map((text, index) => ({
    offset: index * 100,
    text,
    cutFrom: 0,
  })),
  start: 0,
  end: 400,
  scannedTo: 0,
  size: 400,
  partial: false,
};

const summary: LogSummary = {
  targets: [
    { target: "xml5ever::tokenizer", lines: 4001 },
    { target: "cadmus_core::library", lines: 210 },
    { target: "cadmus_core::device::wifi", lines: 48 },
  ],
  levels: [
    { level: "info", lines: 300 },
    { level: "debug", lines: 3900 },
    { level: "warn", lines: 40 },
    { level: "error", lines: 3 },
    { level: "trace", lines: 0 },
  ],
  lines: 4283,
  firstTimestamp: "2026-11-16T07:00:00.000000Z",
  lastTimestamp: "2026-11-16T07:42:11.000000Z",
  scanned: 4096,
  size: 4096,
  done: true,
};

function apiWith(overrides: Partial<CompanionLogsApi>): CompanionLogsApi {
  return {
    listLogs: async () => [currentRun, olderRun, launcherLog],
    readLogPage: async () => fullPage,
    summarizeLog: async (_token, _id, onSummary) => onSummary(summary),
    downloadLogs: async (_token, _ids, onEvent) => {
      const event: LogDownloadEvent = {
        read: 1024,
        total: 4096,
        data: new ArrayBuffer(0),
        fileName: "cadmus-logs-20261116T070000Z.zip",
      };
      onEvent(event);
    },
    ...overrides,
  };
}

const scanningPage: LogPage = {
  ...fullPage,
  lines: fullPage.lines.slice(0, 2),
  start: 200,
  end: 400,
  scannedTo: 200,
  partial: true,
};

const scanningApi = apiWith({
  readLogPage: async (_token: string, request: LogReadRequest) =>
    request.cursor === 200 ? new Promise<LogPage>(() => {}) : scanningPage,
});

const cutPage: LogPage = {
  ...fullPage,
  lines: [
    ...fullPage.lines,
    {
      offset: 500,
      text: JSON.stringify({
        timestamp: "2026-11-16T07:00:11.000000Z",
        level: "INFO",
        fields: { message: "a line too long to send whole" },
        target: "cadmus_core::app",
      }),
      cutFrom: 92_000,
    },
  ],
  end: 600,
  size: 600,
};

const errorApi = apiWith({
  listLogs: async () => {
    throw new Error("Companion request failed: unavailable: api stopped");
  },
});

const PAGING_LINE_BYTES = 120;
const PAGING_LINE_COUNT = 240;
const PAGING_PAGE_SIZE = 40;

function pagingLine(index: number): LogLine {
  const lengths = [40, 120, 240, 480, 80, 320];
  const message = "x".repeat(lengths[index % lengths.length]);
  return {
    offset: index * PAGING_LINE_BYTES,
    text: JSON.stringify({
      timestamp: "2026-11-27T07:00:00.000000Z",
      level: index % 7 === 0 ? "WARN" : "INFO",
      fields: { message },
      target: "cadmus_core::paging",
    }),
    cutFrom: 0,
  };
}

function pagingPage(request: LogReadRequest): LogPage {
  const total = PAGING_LINE_COUNT * PAGING_LINE_BYTES;
  const span = PAGING_PAGE_SIZE * PAGING_LINE_BYTES;
  let start: number;
  let end: number;
  if (request.fromEnd) {
    end = total;
    start = Math.max(0, total - span);
  } else if (request.direction === "backward") {
    end = request.cursor;
    start = Math.max(0, end - span);
  } else {
    start = request.cursor;
    end = Math.min(total, start + span);
  }
  const first = Math.floor(start / PAGING_LINE_BYTES);
  const last = Math.ceil(end / PAGING_LINE_BYTES);
  const lines: LogLine[] = [];
  for (let index = first; index < last; index += 1) {
    lines.push(pagingLine(index));
  }
  return {
    lines,
    start,
    end,
    scannedTo: end,
    size: total,
    partial: false,
  };
}

const pagingLog: LogFile = {
  ...currentRun,
  id: "paging",
  name: "paging.json",
  size: PAGING_LINE_COUNT * PAGING_LINE_BYTES,
  current: false,
  runId: "paging",
};

const pagingApi = apiWith({
  readLogPage: async (_token: string, request: LogReadRequest) =>
    pagingPage(request),
});

const meta: Meta<typeof CompanionLogs> = {
  title: "Components/CompanionLogs",
  component: CompanionLogs,
  args: { token: "123456", api: apiWith({}) },
};

export default meta;
type Story = StoryObj<typeof CompanionLogs>;

export const List: Story = {};

export const Selected: Story = {
  args: { initialSelected: [olderRun.id, launcherLog.id] },
};

export const Empty: Story = {
  args: { api: apiWith({ listLogs: async () => [] }) },
};

export const ListError: Story = {
  args: { api: errorApi },
};

export const Viewer: Story = {
  args: { initialLog: currentRun },
};

export const ViewerScanning: Story = {
  args: { initialLog: currentRun, api: scanningApi },
};

export const SummaryScanning: Story = {
  args: {
    initialLog: currentRun,
    api: apiWith({
      summarizeLog: async (_token, _id, onSummary) =>
        onSummary({ ...summary, scanned: 1024, done: false }),
    }),
  },
};

export const CutLine: Story = {
  args: {
    initialLog: currentRun,
    api: apiWith({ readLogPage: async () => cutPage }),
  },
};

export const OlderRun: Story = {
  args: { initialLog: olderRun },
};

export const ViewerExpanded: Story = {
  args: { initialLog: currentRun },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const [entry] = canvas.getAllByRole("button", { name: /Show entry/ });
    await userEvent.click(entry);
  },
};

export const ViewerSelecting: Story = {
  args: { initialLog: currentRun },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "Select lines" }));
    await userEvent.click(canvas.getByText("Cadmus started with ID test"));
  },
};

export const DownloadProgress: Story = {
  args: {
    initialSelected: [olderRun.id],
    api: apiWith({
      downloadLogs: async (_token, _ids, onEvent) => {
        onEvent({
          read: 32 * 1024 * 1024,
          total: 64 * 1024 * 1024,
          data: new ArrayBuffer(0),
          fileName: "cadmus-logs-20261116T070000Z.zip",
        });
        await new Promise<void>(() => {});
      },
    }),
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "Download" }));
  },
};

export const DownloadBusy: Story = {
  args: {
    initialSelected: [olderRun.id],
    api: apiWith({
      downloadLogs: async () => {
        throw Object.assign(new Error("another download is running"), {
          status: 500,
          code: "resource_exhausted",
        });
      },
    }),
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "Download" }));
  },
};

export const Paging: Story = {
  args: { initialLog: pagingLog, api: pagingApi },
  decorators: [
    (Story) => (
      <div
        style={{ height: "100vh", display: "flex", flexDirection: "column" }}
      >
        <Story />
      </div>
    ),
  ],
};
