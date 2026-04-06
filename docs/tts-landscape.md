# TTS Landscape

Reviewed on 2026-04-05.

This version is intentionally domestic-first.

It focuses on the TTS options that matter most for SpeechMesh if the priority is:

- Chinese quality
- controllable emotion, speaking rate, or style
- self-hosting feasibility
- clear commercial/license boundaries
- a realistic split between online APIs and local deployment

## Bottom Line

- Truly free online TTS does not really exist. What exists is free quota, free trial, or evaluation credit.
- If you want strong controllability and long-term cost control, local deployment is the real path.
- The domestic open-source models that matter most right now are:
  - `Qwen3-TTS`
  - `CosyVoice 3.x`
  - `GLM-TTS`
  - `Spark-TTS`
  - `IndexTTS2`
  - `FireRedTTS2`
- If you want explicit emotion and speed control, the strongest domestic shortlist is:
  - `Qwen3-TTS`
  - `CosyVoice 3.x` / Aliyun CosyVoice service
  - `GLM-TTS`
  - `EmotiVoice`
- If you want a practical first implementation order for SpeechMesh, the best order is:
  1. `qwen3-tts`
  2. `cosyvoice`
  3. `glm-tts` or `spark-tts`
  4. `melo` as a lightweight fallback
  5. optional managed online providers: `aliyun-qwen3-tts`, `aliyun-cosyvoice`, `tencent-tts`, `volcengine-tts`, `minimax-speech`

## Domestic Local Deployment Models

### Primary candidates

| Model | Org | Control surface | Streaming | Voice clone / design | License | Recommendation |
| --- | --- | --- | --- | --- | --- | --- |
| Qwen3-TTS | Alibaba Cloud / Qwen | Natural-language voice control, expressive speech, custom voice, voice design | Yes | Yes / Yes | Apache-2.0 | Best all-round domestic open model right now |
| CosyVoice 3.x | FunAudioLLM / Alibaba ecosystem | Emotion, scene, role, dialect, SSML, pronunciation control | Yes | Yes / Yes on service side | Apache-2.0 | Best Chinese production-style baseline |
| GLM-TTS | Zhipu / Z.ai | Explicit emotion-expressive design, RL-enhanced expressiveness, phoneme control | Yes | Zero-shot cloning | Apache-2.0 | Best research-grade expressive candidate |
| Spark-TTS | SparkAudio | Controllable gender, pitch, speaking rate | Not positioned as dialogue streaming first | Zero-shot clone | Apache-2.0 | Strong controllable alternative |

### Secondary but important candidates

| Model | Org | Strength | Main limit | License | Recommendation |
| --- | --- | --- | --- | --- | --- |
| IndexTTS2 | Bilibili IndexTTS | Industrial controllability, emotion-text guidance, strong emotional fidelity claims | Commercial cooperation posture should be checked case by case | Apache-2.0 | Very worth testing |
| FireRedTTS2 | FireRedTeam | Long conversational speech, multi-speaker dialogue, low-latency streaming | Better for podcast/chatbot than generic single-speaker TTS | Apache-2.0 | Add if SpeechMesh wants dialogue-native TTS |
| EmotiVoice | NetEase Youdao | Prompt-controlled emotion, speed support, easy to reason about | Older stack, naturalness may lag newer models | Apache-2.0 | Good control-focused provider |
| GPT-SoVITS | RVC-Boss | Very practical cloning ecosystem, Chinese community adoption, speed control | Emotion control is still weaker than the best new models | MIT | Good optional cloning-oriented provider |
| MeloTTS | MyShell | Lightweight, multilingual, CPU friendly | Emotion/style control is limited | MIT | Best lightweight fallback |
| OpenVoice V2 | MyShell | Fine-grained style control: emotion, accent, rhythm, pauses, intonation | Better as style-transfer provider than default TTS | MIT | Optional specialized provider |

## Domestic Model Notes

### 1. Qwen3-TTS

Why it matters:

- open-sourced by Qwen with Apache-2.0
- official support for streaming speech generation, voice clone, and voice design
- natural-language instruction control is first-class, not an afterthought
- official examples show emotion-style instructions directly in the API, for example angry or panicked tone descriptions
- available in `0.6B` and `1.7B` families, which is useful for different deployment budgets

Best fit:

- the primary local provider for SpeechMesh if you want one modern, expressive, domestic default

Trade-offs:

- heavier than lightweight baselines
- still needs serious GPU planning for production throughput

### 2. CosyVoice 3.x

Why it matters:

- still one of the strongest Chinese-first TTS families
- wide language plus Chinese dialect coverage
- official online docs expose structured instruction control for emotion, scene, role, and identity
- service docs also expose SSML and strong production alignment

Best fit:

- the most conservative and production-friendly domestic Chinese choice
- especially good if dialects matter

Trade-offs:

- open local repo and managed online service do not expose exactly the same surface area
- for maximum expressiveness, Qwen3-TTS is now a serious competitor, not just a complement

### 3. GLM-TTS

Why it matters:

- officially positions itself as controllable and emotion-expressive zero-shot TTS
- uses RL to improve emotional expression
- supports phoneme-level control, which is useful for pronunciation-sensitive products
- Chinese is the primary language focus

Best fit:

- expressive TTS where emotional quality is a first-order requirement
- a strong candidate for an advanced SpeechMesh provider

Trade-offs:

- newer ecosystem and less battle-tested operationally than CosyVoice

### 4. Spark-TTS

Why it matters:

- official control over gender, pitch, and speaking rate
- Chinese/English support
- zero-shot voice cloning
- Triton deployment path is already published

Best fit:

- a controllable provider where prosody knobs matter more than voice-design features

Trade-offs:

- current public materials make pitch/rate control clear, but not a strong explicit emotion story like Qwen3-TTS or GLM-TTS

### 5. IndexTTS2

Why it matters:

- positions itself as industrial-level controllable zero-shot TTS
- official repo exposes `use_emo_text` and `emo_text`-style emotion guidance
- claims strong emotional fidelity in public evaluation notes

Best fit:

- worth benchmarking as a serious controllable Chinese provider

Trade-offs:

- I would benchmark it before choosing it as the default provider for SpeechMesh

### 6. FireRedTTS2

Why it matters:

- long-form, multi-speaker, dialogue-native TTS
- context-aware prosody
- streaming generation with low first-packet latency claims
- excellent fit for podcast or agent conversation output

Best fit:

- podcast, multi-role conversation, spoken-agent products

Trade-offs:

- overkill for simple single-speaker TTS

### 7. EmotiVoice

Why it matters:

- explicit prompt-controlled emotion
- official speed control support in its API path
- easier provider semantics for a generic abstraction layer

Best fit:

- a clear control-model provider in SpeechMesh
- useful for proving out `emotion` and `rate` options in a provider-neutral API

Trade-offs:

- quality may not beat the newest large models

### 8. GPT-SoVITS

Why it matters:

- huge Chinese community adoption
- strong few-shot cloning story
- practical training and WebUI workflow
- official speed control support

Best fit:

- cloning-heavy scenarios
- maker/creator workflows

Trade-offs:

- explicit emotion control is not yet its strongest official story

## Domestic Online Services

Online providers are not really free; treat them as trial-friendly or low-volume options.

### Domestic online shortlist

| Service | Free status | Control surface | Protocol / access | Recommendation |
| --- | --- | --- | --- | --- |
| Aliyun Qwen3-TTS Realtime | Official docs show model-specific free quota / newcomer quota, varies by model and region | Natural-language instruction control for emotion, role, tone, speed, pitch | WebSocket + SDK | Best domestic online expressive API |
| Aliyun CosyVoice | Official docs show per-model free quota; some clone flows are free while synthesis is billed | Emotion, contextual prosody, SSML, dialects, clone/design on some models | WebSocket + API | Best domestic online Chinese baseline |
| Tencent Cloud TTS | Official docs say free quota is available as free resource pack | SSML, speed, volume, emotion in streaming path | HTTP + WebSocket + SDK | Best conservative cloud fallback |
| Volcengine Doubao Speech | Official docs and billing pages mention trial quota / console-issued trial | Short text, long text, emotion-prediction version, online/offline SDK paths | HTTP + WebSocket + SDK | Strong product option, especially if you already use ByteDance stack |
| MiniMax Speech | Free trial exists, but quota details are less cleanly exposed than the others and should be confirmed in console | Speed, pitch, volume, voice design, cloning, expressive paralinguistic tags | HTTP + WebSocket | Strong expressive API, but not my first pick for "free" |

## Domestic Online Notes

### 1. Aliyun Qwen3-TTS Realtime

What stands out:

- instruct model explicitly supports natural-language control over tone, speed, emotion, and character style
- supports streaming input and streaming output
- official docs expose multiple real-time variants for instruct, voice design, and voice clone

Practical judgment:

- if you want the strongest domestic online TTS API with modern control semantics, this is the first thing to test

### 2. Aliyun CosyVoice

What stands out:

- official quick-start docs describe contextual emotion/prosody behavior
- current pricing pages show model-level free quota
- official voice list pages expose instructable emotion values and scene/role settings on supported voices

Practical judgment:

- if your first KPI is Chinese naturalness, readability, and operational stability, this is still one of the safest choices

### 3. Tencent Cloud TTS

What stands out:

- official docs explicitly say free calling quota exists and must be claimed in the console
- official APIs support SSML, speed, volume, and in streaming mode also emotion parameters
- available over WebSocket and conventional API paths

Practical judgment:

- not the most cutting-edge expressive stack, but very practical and integration-friendly

### 4. Volcengine Doubao Speech

What stands out:

- official docs expose short-text, long-text, and emotion-prediction versions
- official docs show both HTTP and WebSocket access
- official SDK overview shows online and offline SDK families

Practical judgment:

- strong option if you want ByteDance ecosystem alignment or mixed online/offline product shapes

### 5. MiniMax Speech

What stands out:

- very strong product-level expressiveness
- official docs expose speed, pitch, volume, streaming output, voice design, cloning, and expressive paralinguistic tags
- widely used in AI-podcast and expressive-agent scenarios

Practical judgment:

- product quality looks strong, but for a strict "free-first" evaluation I would place it behind Aliyun and Tencent because the free-quota posture is less straightforward in public docs

## Global Open Models Worth Keeping In Scope

These are still useful, but they should now be treated as supplements, not the main domestic recommendation:

- `Chatterbox`: strong expressive multilingual model, MIT
- `Kokoro-82M`: tiny and cheap fallback, Apache-2.0
- `Parler-TTS`: promptable style control, but weaker than the domestic front-runners for Chinese

## License Risk Notes

These are interesting, but I would not make them default providers without a deliberate policy decision:

- `F5-TTS`: code is MIT, but official pretrained models are non-commercial
- `Fish Speech`: repo license is not as clean as Apache/MIT for a default enterprise story
- `XTTS-v2`: model license is more restrictive than Apache/MIT defaults
- `ChatTTS`: code and model licensing is not ideal for a clean commercial default

## What This Means For SpeechMesh

Recommended implementation order:

1. `qwen3-tts`
2. `cosyvoice`
3. `glm-tts`
4. `spark-tts` or `indextts2`
5. `melo`
6. optional cloud adapters: `aliyun-qwen3-tts`, `aliyun-cosyvoice`, `tencent-tts`, `volcengine-tts`, `minimax-speech`
7. optional specialized providers: `fireredtts2`, `openvoice`, `gpt-sovits`

Recommended provider capability fields:

- `rate`
- `pitch`
- `volume`
- `emotion`
- `emotion_mode`
- `style`
- `role`
- `speaker`
- `speaker_reference`
- `voice_design`
- `language`
- `dialect`
- `streaming`
- `ssml`

Recommended `emotion_mode` values:

- `none`
- `preset`
- `natural_language_instruct`
- `emotion_text_guidance`
- `ssml_style`

Important design note:

- providers do not mean the same thing by "emotion"
- some providers support explicit emotion labels
- some support only prompt-level style descriptions
- some support only coarse prosody knobs like pitch/rate
- SpeechMesh should expose capability bits and provider-specific option passthroughs instead of forcing fake uniformity

## My Actual Ranking For Your Use Case

If your real requirement is "free or nearly free, can tune emotion/speaking style/speed, and should work as a serious backend service", my ranking is:

### Local deployment

1. `Qwen3-TTS`
2. `CosyVoice 3.x`
3. `GLM-TTS`
4. `Spark-TTS`
5. `IndexTTS2`
6. `EmotiVoice`
7. `FireRedTTS2` for dialogue/podcast
8. `MeloTTS` for lightweight fallback

### Online APIs

1. `Aliyun Qwen3-TTS Realtime`
2. `Aliyun CosyVoice`
3. `Tencent Cloud TTS`
4. `Volcengine Doubao Speech`
5. `MiniMax Speech`

## Sources

Primary official sources used in this review:

- Qwen3-TTS GitHub: https://github.com/QwenLM/Qwen3-TTS
- Qwen3-TTS realtime docs: https://help.aliyun.com/zh/model-studio/qwen-tts-realtime
- Qwen-TTS model docs: https://help.aliyun.com/document_detail/2975508.html
- Aliyun newcomer free quota docs: https://help.aliyun.com/document_detail/2975577.html
- CosyVoice GitHub: https://github.com/FunAudioLLM/CosyVoice
- CosyVoice quick start and pricing: https://help.aliyun.com/model-studio/developer-reference/quick-start-cosyvoice
- CosyVoice voice list and instruct controls: https://help.aliyun.com/zh/model-studio/cosyvoice-voice-list
- CosyVoice clone/design API: https://help.aliyun.com/zh/model-studio/cosyvoice-clone-design-api
- GLM-TTS GitHub: https://github.com/zai-org/GLM-TTS
- Spark-TTS GitHub: https://github.com/SparkAudio/Spark-TTS
- IndexTTS GitHub: https://github.com/index-tts/index-tts
- FireRedTTS2 GitHub: https://github.com/FireRedTeam/FireRedTTS2
- EmotiVoice GitHub: https://github.com/netease-youdao/EmotiVoice
- GPT-SoVITS GitHub: https://github.com/RVC-Boss/GPT-SoVITS
- MeloTTS GitHub: https://github.com/myshell-ai/MeloTTS
- OpenVoice GitHub: https://github.com/myshell-ai/OpenVoice
- Tencent Cloud TTS overview: https://cloud.tencent.com/document/product/1073/34087
- Tencent long-text TTS API: https://cloud.tencent.com/document/product/1073/57373
- Tencent streaming TTS: https://cloud.tencent.com/document/product/1073/108595
- Tencent pricing/free quota overview: https://cloud.tencent.com/document/product/1073/34112
- Volcengine Doubao Speech SDK overview: https://www.volcengine.com/docs/6561/79827
- Volcengine TTS API docs: https://www.volcengine.com/docs/6489/81406
- Volcengine premium long-text TTS docs: https://www.volcengine.com/docs/6561/1096680
- Volcengine billing overview: https://www.volcengine.com/docs/6561/1359369
- MiniMax speech T2A docs: https://platform.minimaxi.com/docs/api-reference/speech-t2a-intro
- MiniMax voice design docs: https://platform.minimaxi.com/docs/api-reference/voice-design-intro
- MiniMax voice package pricing: https://platform.minimaxi.com/docs/pricing/audio-package
- MiniMax WebSocket TTS: https://platform.minimaxi.com/docs/api-reference/speech-t2a-websocket

## Review Notes

- This is an integration recommendation, not a benchmark claim.
- For online providers, free quota and trial rules can change; re-check pricing before choosing a default.
- For SpeechMesh, the most important omission in the earlier draft was domestic current-generation models. This document fixes that by making Qwen3-TTS, CosyVoice, GLM-TTS, Spark-TTS, and IndexTTS2 the center of the evaluation.
