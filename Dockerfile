# Based on https://aws.amazon.com/blogs/compute/creating-a-simple-fetch-and-run-aws-batch-job/

FROM rust:latest
RUN apt update -y && apt install -y awscli zstd tar
ADD fetch_and_run.sh /usr/local/bin/fetch_and_run.sh
RUN chmod +x /usr/local/bin/fetch_and_run.sh

ENTRYPOINT [ "/usr/local/bin/fetch_and_run.sh" ]
