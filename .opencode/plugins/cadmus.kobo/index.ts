import { Plugin } from "@opencode/plugin";
import { createDeployTool } from "./deploy.ts";
import { createDevicesTool } from "./devices.ts";
import { restartTool } from "./restart.ts";

export default Plugin.define({
  id: "cadmus.kobo",
  async setup(ctx) {
    await ctx.tool.transform((editor) => {
      editor.namespace({
        name: "cadmus.kobo",
        description:
          "Build, deploy and restart Cadmus on a Kobo device over SSH",
      });
      editor.add(createDevicesTool(ctx.location.directory));
      editor.add(createDeployTool(ctx.location.directory));
      editor.add(restartTool);
    });
  },
});
