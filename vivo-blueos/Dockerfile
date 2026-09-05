FROM zjpzjp/blueos:latest

RUN apt-get update && apt-get install -y --no-install-recommends \
    fish \
    vim \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://claude.ai/install.sh | bash

RUN curl -fsSL https://chatgpt.com/codex/install.sh | sh

RUN apt-get update && apt-get install -y --no-install-recommends \
    wget \
    && rm -rf /var/lib/apt/lists/*

ARG LOCAL_BIN="/root/.local/bin"

RUN wget https://github.com/dandavison/delta/releases/download/0.19.2/delta-0.19.2-x86_64-unknown-linux-gnu.tar.gz \
  && tar xvf delta-0.19.2-x86_64-unknown-linux-gnu.tar.gz \
  && mv delta-0.19.2-x86_64-unknown-linux-gnu/delta ${LOCAL_BIN} \
  && rm -rf delta-0.19.2-x86_64-unknown-linux-gnu/ \
  && rm delta-0.19.2-x86_64-unknown-linux-gnu.tar.gz

# claude and codex are install in `~/.local/bin`
ENV PATH="${LOCAL_BIN}:${PATH}"

# RUN curl -LsSf https://astral.sh/uv/install.sh | sh \
#   && uv tool install --upgrade 'cocoindex-code[slim]' \
#   && claude mcp add cocoindex-code -- ccc mcp \
#   && codex mcp add cocoindex-code -- ccc mcp

RUN wget https://github.com/BurntSushi/ripgrep/releases/download/15.2.0/ripgrep-15.2.0-x86_64-unknown-linux-musl.tar.gz \
  && tar xvf ripgrep-15.2.0-x86_64-unknown-linux-musl.tar.gz \
  && mv ripgrep-15.2.0-x86_64-unknown-linux-musl/rg ${LOCAL_BIN} \
  && rm -rf ripgrep-15.2.0-x86_64-unknown-linux-musl/ \
  && rm ripgrep-15.2.0-x86_64-unknown-linux-musl.tar.gz

RUN RUST_SRC=$(rustc --print sysroot)/lib/rustlib/src \
  && mkdir -p ${RUST_SRC} \
  && git clone --depth 1 https://github.com/vivoblueos/rust.git ${RUST_SRC}/rust \
  && cd ${RUST_SRC}/rust \
  && git submodule update --init --recursive --depth 1 library/backtrace \
  && git submodule update --init --recursive --depth 1 library/stdarch

RUN echo "fish" >> /root/.bashrc

