FROM zjpzjp/blueos:nightly-2026-07-21_v3

RUN apt-get update && apt-get install -y --no-install-recommends \
    fish \
    vim \
    wget \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://claude.ai/install.sh | bash

RUN curl -fsSL https://chatgpt.com/codex/install.sh | sh

ARG LOCAL_BIN=/root/.local/bin
ARG CARGO_BIN=/root/.cargo/bin

RUN mkdir -p ${LOCAL_BIN} && mkdir -p ${CARGO_BIN}

RUN wget https://github.com/dandavison/delta/releases/download/0.19.2/delta-0.19.2-x86_64-unknown-linux-gnu.tar.gz \
  && tar xvf delta-0.19.2-x86_64-unknown-linux-gnu.tar.gz \
  && mv delta-0.19.2-x86_64-unknown-linux-gnu/delta ${LOCAL_BIN} \
  && rm -rf delta-0.19.2-x86_64-unknown-linux-gnu/ \
  && rm delta-0.19.2-x86_64-unknown-linux-gnu.tar.gz

# claude and codex are install in `~/.local/bin`
ENV PATH="/root/.cache/miri/bin:${CARGO_BIN}:${LOCAL_BIN}:${PATH}"

RUN wget https://github.com/BurntSushi/ripgrep/releases/download/15.2.0/ripgrep-15.2.0-x86_64-unknown-linux-musl.tar.gz \
  && tar xvf ripgrep-15.2.0-x86_64-unknown-linux-musl.tar.gz \
  && mv ripgrep-15.2.0-x86_64-unknown-linux-musl/rg ${LOCAL_BIN} \
  && rm -rf ripgrep-15.2.0-x86_64-unknown-linux-musl*

RUN wget https://github.com/sharkdp/fd/releases/download/v10.5.0/fd-v10.5.0-x86_64-unknown-linux-gnu.tar.gz \
  && tar xvf fd-v10.5.0-x86_64-unknown-linux-gnu.tar.gz \
  && mv fd-v10.5.0-x86_64-unknown-linux-gnu/fd ${LOCAL_BIN} \
  && rm -rf fd-v10.5.0-x86_64-unknown-linux-gnu*

RUN echo "fish" >> /root/.bashrc

WORKDIR /KMiri/vivo-blueos
