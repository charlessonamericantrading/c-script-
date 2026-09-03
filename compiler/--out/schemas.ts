// Generado automáticamente por linkc v1.200.1 — no editar a mano.

import { z } from "zod";

export const PdfBlockSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("Text"), content: z.string(), bold: z.boolean(), size: z.number().int() }),
  z.object({ type: z.literal("Table"), headers: z.array(z.string()), rows: z.array(z.array(z.string())) })
]);
export type PdfBlock = z.infer<typeof PdfBlockSchema>;

export const ExcelCellSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("Text"), value: z.string() }),
  z.object({ type: z.literal("Number"), value: z.string().regex(/^-?\d+\.\d{4}$/) }),
  z.object({ type: z.literal("Date"), value: z.string().datetime() }),
  z.object({ type: z.literal("Bool"), value: z.boolean() }),
  z.object({ type: z.literal("Empty") })
]);
export type ExcelCell = z.infer<typeof ExcelCellSchema>;

export const ExcelSheetSchema = z.object({
  name: z.string(),
  headers: z.array(z.string()),
  rows: z.array(z.array(ExcelCellSchema)),
});
export type ExcelSheet = z.infer<typeof ExcelSheetSchema>;

export const AiMessageSchema = z.object({
  role: z.string(),
  content: z.string(),
});
export type AiMessage = z.infer<typeof AiMessageSchema>;

export const AiTokenSchema = z.object({
  token: z.string(),
  done: z.boolean(),
});
export type AiToken = z.infer<typeof AiTokenSchema>;

export const AuthorSchema = z.object({
  id: z.number().int(),
  name: z.string(),
});
export type Author = z.infer<typeof AuthorSchema>;

export const PostSchema = z.object({
  id: z.number().int(),
  title: z.string(),
  authorId: z.number().int(),
});
export type Post = z.infer<typeof PostSchema>;

