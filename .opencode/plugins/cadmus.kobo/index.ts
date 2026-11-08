import { Plugin } from "@opencode/plugin";
import { createDeployTool } from "./deploy.ts";
import { createRestartTool } from "./restart.ts";

export default Plugin.define({
  id: "cadmus.kobo",
  async setup(ctx) {
    await ctx.tool.transform((editor) => {
      editor.namespace({
        name: "cadmus.kobo",
        description:
          "Build, deploy and restart Cadmus on a Kobo device over SSH",
      });
      editor.add(createDeployTool(ctx.location.directory));
      editor.add(createRestartTool(ctx.location.directory));
    });
  },
});
