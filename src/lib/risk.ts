import type { Risk } from "../bindings/Risk";

export const RISK: Record<Risk, string> = {
  safe: "Safe",
  lowRisk: "Low risk",
  mediumRisk: "Medium risk",
  highRisk: "High risk",
  protected: "Protected",
};
