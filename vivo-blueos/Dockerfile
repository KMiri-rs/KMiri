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

RUN wget https://github.com/dandavison/delta/releases/download/0.19.2/delta-0.19.2-x86_64-unknown-linux-gnu.tar.gz \
  && tar xvf delta-0.19.2-x86_64-unknown-linux-gnu.tar.gz \
  && mv delta-0.19.2-x86_64-unknown-linux-gnu/delta /root/.local/bin \
  && rm -rf delta-0.19.2-x86_64-unknown-linux-gnu/

RUN echo "fish" >> /root/.bashrc

# claude and codex are install in `~/.local/bin`
ENV PATH="/root/.local/bin:${PATH}"
