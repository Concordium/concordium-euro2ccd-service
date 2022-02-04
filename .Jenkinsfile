pipeline {
    agent any
    environment {
        S3_BUCKET = "s3://euro2ccd.concordium.com"
        OUT_FILE = "euro2ccd-${tag}.deb"
        OUT_PATH = "${S3_BUCKET}/${OUT_FILE}"
        OUT_DIR = sh(script: 'mktemp -d', returnStdout: true).trim()
    }
    stages {
        
        stage('Build') {
            steps {
                sh '''\
                    docker build -t ccd-service-builder --build-arg ubuntu_version=${base_image} -f scripts/debian-package/deb.Dockerfile .

                    set -euxo pipefail

                    id=$(docker create ccd-service-builder)
                    docker cp $id:/build/pkg-root/ $outfile
                    docker rm $id

                '''.stripIndent()
                stash includes: '/${OUT_FILE}', name: 'built'
            }
        }
       stage('Publish') {
            steps {
                unstash 'built'
                sh '''\
                    # Push to S3.
                    aws s3 cp ${OUT_FILE} "${OUT_PATH}" 
                '''.stripIndent()
            }
        }
}
