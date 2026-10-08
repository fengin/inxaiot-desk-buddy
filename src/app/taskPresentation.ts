import { aioTaskPresentation } from "@/features/aio/taskPresentation";
import { screenTaskPresentation } from "@/features/smart-screen/taskPresentation";
import { registerTaskPresentation } from "@/shared/model/taskPresentation";

registerTaskPresentation("aio", aioTaskPresentation);
registerTaskPresentation("smart_screen", screenTaskPresentation);

export { taskStageLabel, canRetryTaskResult } from "@/shared/model/taskPresentation";
