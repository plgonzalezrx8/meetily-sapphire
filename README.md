# Meetily Sapphire

![A sapphire prism transforming an incoming audio waveform into outgoing note strokes](docs/sapphire/prism.webp)

Meetily Sapphire is a free, open-source desktop meeting assistant. It records microphone and system audio, transcribes the conversation on your computer, and turns the transcript into meeting notes. You choose where the summary runs.

These notes come from a fictional delivery meeting summarized on Windows with GPT-6-Luna through a connected ChatGPT plan.

> **Decision** Keep the fictional launch scheduled for Friday.
>
> **Action** Alex will update the checklist tomorrow.

## Build from source

Build from the fork’s `Development` branch to try the latest integrated features. Follow the platform prerequisites and prepare the `llama-helper` sidecar before starting the app. [Development branch](https://github.com/plgonzalezrx8/meetily-sapphire/tree/Development) · [Build guide](docs/BUILDING.md)

Once the prerequisites and sidecar are ready, from `frontend/` run:

```sh
pnpm install --frozen-lockfile
pnpm run tauri:dev
```

Use pnpm 9.15.9. For a production build, follow the release sidecar instructions in the build guide, then run `pnpm run tauri:build` from `frontend/`. Platform details are in the [Linux](docs/building_in_linux.md) and [GPU acceleration](docs/GPU_ACCELERATION.md) guides.

## Choose where summaries run

<img src="docs/sapphire/data-flow.svg" width="480" alt="Data flow from meeting audio to on-device transcription and a choice of local or cloud summary providers.">

Whisper and Parakeet transcribe audio on your device. Recordings, transcripts, and completed summaries are stored locally. For offline summaries, select a built-in model or a locally running Ollama model after downloading its weights.

For hosted summaries, open **Settings → Summary** and find **Summary Model Configuration**. The selected provider receives the transcript text and summary instructions.

- **OpenAI API key** uses your OpenAI API account. Sapphire suggests `gpt-6-luna` for everyday summaries and `gpt-6.2-sol` for difficult transcripts.
- **ChatGPT plan** uses the allowance on an eligible ChatGPT account. OpenAI supplies its model list. **Manage usage** opens your ChatGPT usage controls. To switch to API billing, choose **OpenAI API key** explicitly. [OpenAI’s plan usage guide](https://developers.openai.com/siwc/token-sharing-open-source)
- **Anthropic API key** uses your Anthropic account. Sapphire suggests `claude-sonnet-5-5` for quality and `claude-haiku-5-5` for lower cost.

For either API key, the picker labels suggestions separately from models found for your account. If your key restricts model listing, enter the exact model ID. You can select a replacement when a saved model retires.

Groq, OpenRouter, and custom OpenAI-compatible servers are also available. An Ollama or custom server can run on another machine, so its configured address determines where the transcript goes. [Audio and storage architecture](docs/architecture.md)

### Generate a summary with your ChatGPT plan

1. In **Settings → Summary**, select **ChatGPT plan**, read the privacy notice, and choose **Continue with ChatGPT**. Complete sign-in in your browser.
2. Return to Sapphire. When you see **Connected**, choose a model from your account’s list and select **Save**.
3. Open a meeting with a transcript and select **Generate Summary**. The completed notes appear beside the transcript and are saved on your computer.

Automatic summaries start off for each connected account. To send transcripts automatically when meetings end, enable **Auto Summary** and **Allow automatic summaries using my ChatGPT plan**. You can turn this permission off or choose **Disconnect** in the same settings.

## Contributing

Feature work goes into `Development` before release on `main`. Use synthetic or redacted meeting material in examples and reports. [Contribution guide](CONTRIBUTING.md) · [Fork pull requests](https://github.com/plgonzalezrx8/meetily-sapphire/pulls)

## License and credits

Meetily Sapphire is available under the [MIT License](LICENSE.md). Copyright © 2024 Zackriya Solutions and contributors as applicable. Thank you to the [upstream Meetily project](https://github.com/Zackriya-Solutions/meetily) and its contributors.

The project also draws on [whisper.cpp](https://github.com/ggerganov/whisper.cpp), [Screenpipe](https://github.com/mediar-ai/screenpipe), and [transcribe-rs](https://github.com/cjpais/transcribe-rs). Thanks to NVIDIA for Parakeet and [istupakov](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx) for its ONNX conversion.
